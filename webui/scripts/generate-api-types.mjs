import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const METHODS = ['get', 'post', 'put', 'patch', 'delete', 'options', 'head', 'trace'];
const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const DEFAULT_SPEC = resolve(SCRIPT_DIR, '../../crates/server/assets/api-docs/openapi.json');
const DEFAULT_OUTPUT = resolve(SCRIPT_DIR, '../src/api/types.ts');

function resolveRef(document, ref) {
	if (typeof ref !== 'string' || !ref.startsWith('#/')) {
		throw new Error(`Only local OpenAPI references are supported: ${ref}`);
	}
	let current = document;
	for (const rawPart of ref.slice(2).split('/')) {
		const part = rawPart.replaceAll('~1', '/').replaceAll('~0', '~');
		if (!current || typeof current !== 'object' || !(part in current)) {
			throw new Error(`Unresolved OpenAPI reference: ${ref}`);
		}
		current = current[part];
	}
	return current;
}

function schemaName(ref) {
	const prefix = '#/components/schemas/';
	return ref.startsWith(prefix) ? ref.slice(prefix.length) : null;
}

export function reachableSchemas(document) {
	const reachable = new Set();

	function visitSchema(schema) {
		if (Array.isArray(schema)) {
			for (const value of schema) visitSchema(value);
			return;
		}
		if (!schema || typeof schema !== 'object') return;
		if (schema.$ref) {
			const name = schemaName(schema.$ref);
			if (name !== null) {
				if (reachable.has(name)) return;
				reachable.add(name);
			}
			visitSchema(resolveRef(document, schema.$ref));
			return;
		}
		for (const value of Object.values(schema)) visitSchema(value);
	}

	function visitParameter(parameter) {
		if (!parameter || typeof parameter !== 'object') return;
		const resolved = parameter.$ref ? resolveRef(document, parameter.$ref) : parameter;
		visitSchema(resolved.schema);
	}

	function visitResponse(response) {
		if (!response || typeof response !== 'object') return;
		const resolved = response.$ref ? resolveRef(document, response.$ref) : response;
		visitSchema(resolved.content?.['application/json']?.schema);
	}

	function visitRequestBody(body) {
		if (!body || typeof body !== 'object') return;
		const resolved = body.$ref ? resolveRef(document, body.$ref) : body;
		visitSchema(resolved.content?.['application/json']?.schema);
	}

	for (const [path, pathItem] of Object.entries(document.paths ?? {})) {
		if (path !== '/api/v1' && !path.startsWith('/api/v1/')) continue;
		for (const parameter of pathItem.parameters ?? []) visitParameter(parameter);
		for (const method of METHODS) {
			const operation = pathItem[method];
			if (!operation || operation['x-websocket']) continue;
			for (const parameter of operation.parameters ?? []) visitParameter(parameter);
			visitRequestBody(operation.requestBody);
			for (const response of Object.values(operation.responses ?? {})) visitResponse(response);
		}
	}
	return reachable;
}

function identifier(name) {
	const converted = name.replace(/[^A-Za-z0-9_$]/g, '_');
	return /^[A-Za-z_$]/.test(converted) ? converted : `_${converted}`;
}

function propertyName(name) {
	return /^[A-Za-z_$][A-Za-z0-9_$]*$/.test(name) ? name : JSON.stringify(name);
}

function literal(value) {
	return JSON.stringify(value);
}

function parenthesize(text) {
	return text.startsWith('{') || text.startsWith('Array<') ? text : `(${text})`;
}

function union(parts) {
	const unique = [...new Set(parts)];
	if (unique.length === 0) return 'unknown';
	if (unique.length === 1) return unique[0];
	return unique.map((part) => (part.includes(' & ') ? parenthesize(part) : part)).join(' | ');
}

function intersection(parts) {
	const unique = [...new Set(parts)];
	if (unique.length === 0) return 'unknown';
	if (unique.length === 1) return unique[0];
	return unique.map((part) => (part.includes(' | ') ? parenthesize(part) : part)).join(' & ');
}

function objectType(schema, indent) {
	const required = new Set(schema.required ?? []);
	const lines = ['{'];
	const childIndent = `${indent}\t`;
	for (const [name, property] of Object.entries(schema.properties ?? {})) {
		const optional = required.has(name) ? '' : '?';
		lines.push(`${childIndent}${propertyName(name)}${optional}: ${typeFor(property, childIndent)};`);
	}
	if (schema.additionalProperties) {
		const valueType = schema.additionalProperties === true
			? 'unknown'
			: typeFor(schema.additionalProperties, childIndent);
		lines.push(`${childIndent}[key: string]: ${valueType};`);
	}
	if (lines.length === 1) lines.push(`${childIndent}[key: string]: unknown;`);
	lines.push(`${indent}}`);
	return lines.join('\n');
}

function baseType(schema, indent) {
	if (!schema || typeof schema !== 'object') return 'unknown';
	if (schema.$ref) {
		const name = schemaName(schema.$ref);
		if (name === null) throw new Error(`Type generation requires a schema reference: ${schema.$ref}`);
		return identifier(name);
	}

	const parts = [];
	if (schema.allOf) parts.push(...schema.allOf.map((part) => typeFor(part, indent)));
	if (schema.oneOf) parts.push(union(schema.oneOf.map((part) => typeFor(part, indent))));
	if (schema.enum) parts.push(union(schema.enum.map(literal)));
	if (schema['x-tuple']) {
		parts.push(`[${schema['x-tuple'].map((part) => typeFor(part, indent)).join(', ')}]`);
	} else if (!schema.allOf && !schema.oneOf && !schema.enum) {
		switch (schema.type) {
			case 'string':
				parts.push('string');
				break;
			case 'integer':
			case 'number':
				parts.push('number');
				break;
			case 'boolean':
				parts.push('boolean');
				break;
			case 'null':
				parts.push('null');
				break;
			case 'array':
				parts.push(`Array<${typeFor(schema.items ?? {}, indent)}>`);
				break;
			case 'object':
				parts.push(objectType(schema, indent));
				break;
			default:
				if (schema.properties || schema.additionalProperties) parts.push(objectType(schema, indent));
				else parts.push('unknown');
		}
	}
	return intersection(parts);
}

function typeFor(schema, indent = '') {
	const base = baseType(schema, indent);
	if (!schema?.nullable || base === 'unknown' || base === 'null') return base;
	return union([base, 'null']);
}

export function generateTypes(document) {
	const schemas = document.components?.schemas ?? {};
	const reachable = reachableSchemas(document);
	const declarations = [...reachable].sort().map((name) => {
		if (!(name in schemas)) throw new Error(`Missing component schema: ${name}`);
		return `export type ${identifier(name)} = ${typeFor(schemas[name])};`;
	});
	return `// Generated from crates/server/assets/api-docs/openapi.json; do not edit.\n\n${declarations.join('\n\n')}\n`;
}

export function generateFile(specPath = DEFAULT_SPEC, outputPath = DEFAULT_OUTPUT) {
	const document = JSON.parse(readFileSync(specPath, 'utf8'));
	const output = generateTypes(document);
	writeFileSync(outputPath, output);
	return output;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
	generateFile(...process.argv.slice(2));
}
