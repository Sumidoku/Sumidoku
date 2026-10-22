import assert from 'node:assert/strict';
import test from 'node:test';
import { generateTypes, reachableSchemas } from '../scripts/generate-api-types.mjs';

function fixture() {
	return {
		openapi: '3.0.3',
		paths: {
			'/api/v1/test': {
				parameters: [{ $ref: '#/components/parameters/Mode' }],
				post: {
					requestBody: {
						content: { 'application/json': { schema: { $ref: '#/components/schemas/Input' } } }
					},
					responses: {
						200: { $ref: '#/components/responses/CompositeResponse' }
					}
				}
			},
			'/api/v1/stream': {
				get: {
					responses: {
						200: { content: { 'application/x-ndjson': { schema: { $ref: '#/components/schemas/NdjsonOnly' } } } }
					}
				}
			},
			'/api/v1/ws': {
				get: {
					'x-websocket': { frames: [{ schema: { $ref: '#/components/schemas/WsOnly' } }] },
					responses: { 101: { description: 'Upgrade' } }
				}
			},
			'/api/v1/image': {
				get: {
					responses: {
						200: { content: { 'image/png': { schema: { $ref: '#/components/schemas/BinaryOnly' } } } }
					}
				}
			},
			'/api/v1/upload': {
				post: {
					requestBody: {
						content: { 'multipart/form-data': { schema: { $ref: '#/components/schemas/MultipartOnly' } } }
					},
					responses: { 204: { description: 'Done' } }
				}
			},
			'/opds/v2': {
				get: {
					responses: {
						200: { content: { 'application/json': { schema: { $ref: '#/components/schemas/OpdsOnly' } } } }
					}
				}
			}
		},
		components: {
			parameters: {
				Mode: { name: 'mode', in: 'query', schema: { $ref: '#/components/schemas/ParameterOnly' } }
			},
			responses: {
				CompositeResponse: {
					description: 'Composite',
					content: { 'application/json': { schema: { $ref: '#/components/schemas/Composite' } } }
				}
			},
			schemas: {
				Input: {
					type: 'object',
					required: ['id'],
					properties: {
						id: { type: 'integer' },
						name: { type: 'string', nullable: true },
						kind: { type: 'string', enum: ['single', 'multi'] },
						values: { type: 'array', items: { type: 'string' } },
						map: { type: 'object', additionalProperties: { type: 'integer' } },
						anything: {},
						pair: {
							type: 'array',
							items: {},
							'x-tuple': [{ type: 'string' }, { type: 'integer', nullable: true }]
						}
					}
				},
				Base: {
					type: 'object',
					required: ['id'],
					properties: { id: { type: 'integer' } }
				},
				Composite: {
					allOf: [
						{ $ref: '#/components/schemas/Base' },
						{
							type: 'object',
							required: ['choice'],
							properties: {
								choice: { oneOf: [{ type: 'string' }, { type: 'number' }], nullable: true }
							}
						}
					]
				},
				ParameterOnly: { type: 'string', enum: ['asc', 'desc'] },
				Unused: { type: 'string' },
				NdjsonOnly: { type: 'string' },
				WsOnly: { type: 'string' },
				BinaryOnly: { type: 'string', format: 'binary' },
				MultipartOnly: { type: 'object' },
				OpdsOnly: { type: 'object' }
			}
		}
	};
}

test('converts REST schemas with exact object, collection, union and tuple types', () => {
	const output = generateTypes(fixture());
	assert.match(output, /export type Input = \{\n\tid: number;\n\tname\?: string \| null;/);
	assert.match(output, /\tkind\?: "single" \| "multi";/);
	assert.match(output, /\tvalues\?: Array<string>;/);
	assert.match(output, /\tmap\?: \{\n\t\t\[key: string\]: number;\n\t\};/);
	assert.match(output, /\tanything\?: unknown;/);
	assert.match(output, /\tpair\?: \[string, number \| null\];/);
	assert.match(output, /export type Composite = Base & \{\n\tchoice: string \| number \| null;\n\};/);
	assert.match(output, /export type ParameterOnly = "asc" \| "desc";/);
});

test('keeps recursive refs and emits declarations deterministically', () => {
	const document = {
		paths: {
			'/api/v1/nodes': {
				get: {
					responses: {
						200: { content: { 'application/json': { schema: { $ref: '#/components/schemas/Node' } } } }
					}
				}
			}
		},
		components: {
			schemas: {
				Node: {
					type: 'object',
					required: ['value', 'children', 'parent'],
					properties: {
						value: { type: 'string' },
						children: { type: 'array', items: { $ref: '#/components/schemas/Node' } },
						parent: { allOf: [{ $ref: '#/components/schemas/Node' }], nullable: true }
					}
				}
			}
		}
	};
	const expected = '// Generated from crates/server/assets/api-docs/openapi.json; do not edit.\n\n' +
		'export type Node = {\n' +
		'\tvalue: string;\n' +
		'\tchildren: Array<Node>;\n' +
		'\tparent: Node | null;\n' +
		'};\n';
	assert.equal(generateTypes(document), expected);
	const reordered = structuredClone(fixture());
	reordered.components.schemas = Object.fromEntries(Object.entries(reordered.components.schemas).reverse());
	assert.equal(generateTypes(reordered), generateTypes(fixture()));
});

test('limits reachability to ordinary JSON REST entry points', () => {
	const document = fixture();
	assert.deepEqual(
		[...reachableSchemas(document)].sort(),
		['Base', 'Composite', 'Input', 'ParameterOnly']
	);
	const output = generateTypes(document);
	for (const excluded of ['Unused', 'NdjsonOnly', 'WsOnly', 'BinaryOnly', 'MultipartOnly', 'OpdsOnly']) {
		assert.doesNotMatch(output, new RegExp(`export type ${excluded}\\b`));
	}
});
