//! Aidoku-style filter groups: a named, persisted rule set compiled into the library
//! query's WHERE clause. Rule rows are validated on write (create/replace), so the
//! evaluator only ever sees well-formed rows.

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;
use axum::Extension;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::sync::Arc;

/// Value-less rule types: at most one row per group (include or exclude), so the
/// tri-state editor maps to exactly one row
const FLAG_RULE_TYPES: [&str; 4] = ["downloaded", "has_unread", "started", "completed"];
/// One value per row; same-type includes are OR'd, excludes apply per value, types AND
const VALUED_RULE_TYPES: [&str; 3] = ["source", "content_rating", "category"];

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct StoredRule {
	pub r#type: String,
	pub value: Option<String>,
	pub exclude: bool,
}

/// A validated rule: flag types carry no value, valued types carry their parsed id/rating
struct Rule {
	r#type: String,
	value: Option<i64>,
	exclude: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleInput {
	r#type: String,
	#[serde(default)]
	value: Option<Value>,
	#[serde(default)]
	exclude: bool,
}

fn rule_json(rule: &StoredRule) -> Value {
	// Valued rules round-trip as JSON numbers; flag rules always serialize null
	let value = rule
		.value
		.as_deref()
		.and_then(|v| v.parse::<i64>().ok())
		.map_or(Value::Null, |v| json!(v));
	json!({
		"type": rule.r#type,
		"value": value,
		"exclude": rule.exclude,
	})
}

fn validate_rule(input: &RuleInput) -> ApiResult<Rule> {
	if FLAG_RULE_TYPES.contains(&input.r#type.as_str()) {
		if input.value.is_some() {
			return Err(ApiError::bad_request(format!("rule {} takes no value", input.r#type)));
		}
		return Ok(Rule {
			r#type: input.r#type.clone(),
			value: None,
			exclude: input.exclude,
		});
	}
	if VALUED_RULE_TYPES.contains(&input.r#type.as_str()) {
		let raw = input
			.value
			.as_ref()
			.ok_or_else(|| ApiError::bad_request(format!("rule {} requires a value", input.r#type)))?;
		// Numbers are the canonical form; numeric strings are accepted for hand-written callers
		let value = match raw {
			Value::Number(n) => n.as_i64(),
			Value::String(s) => s.trim().parse::<i64>().ok(),
			_ => None,
		}
		.ok_or_else(|| ApiError::bad_request(format!("invalid rule value: {raw}")))?;
		let ok = match input.r#type.as_str() {
			"content_rating" => (1..=3).contains(&value),
			// no 0 arm: the uncategorized view stays an ad-hoc categoryId/category=0 filter
			_ => value > 0,
		};
		if !ok {
			return Err(ApiError::bad_request(format!(
				"invalid {} rule value: {value}",
				input.r#type
			)));
		}
		return Ok(Rule {
			r#type: input.r#type.clone(),
			value: Some(value),
			exclude: input.exclude,
		});
	}
	Err(ApiError::bad_request(format!("invalid rule type: {}", input.r#type)))
}

/// A (type, value) pair may appear once per group — including both arms at once is a
/// contradiction; the DB's unique index doesn't see NULL flag values, so dedupe in code
fn validate_rules(inputs: &[RuleInput]) -> ApiResult<Vec<Rule>> {
	let mut seen: HashSet<(String, Option<i64>)> = HashSet::new();
	let mut out = Vec::with_capacity(inputs.len());
	for input in inputs {
		let rule = validate_rule(input)?;
		if !seen.insert((rule.r#type.clone(), rule.value)) {
			return Err(ApiError::bad_request(format!(
				"duplicate rule: {} {:?}",
				rule.r#type, rule.value
			)));
		}
		out.push(rule);
	}
	Ok(out)
}

#[derive(Debug, sqlx::FromRow)]
struct GroupRow {
	id: i64,
	name: String,
	sort_order: i64,
}

async fn group_row(pool: &SqlitePool, user_id: i64, id: i64) -> ApiResult<GroupRow> {
	let row: Option<GroupRow> = sqlx::query_as(
		"SELECT id, name, sort_order FROM filter_group WHERE id = ? AND user_id = ?",
	)
	.bind(id)
	.bind(user_id)
	.fetch_optional(pool)
	.await?;
	row.ok_or_else(|| ApiError::not_found("filter group not found"))
}

async fn rules_of(pool: &SqlitePool, group_id: i64) -> ApiResult<Vec<StoredRule>> {
	let rules = sqlx::query_as(
		"SELECT type, value, exclude FROM filter_group_rule WHERE group_id = ? ORDER BY id ASC",
	)
	.bind(group_id)
	.fetch_all(pool)
	.await?;
	Ok(rules)
}

fn group_json(row: &GroupRow, rules: &[StoredRule]) -> Value {
	json!({
		"id": row.id,
		"name": row.name,
		"sortOrder": row.sort_order,
		"rules": rules.iter().map(rule_json).collect::<Vec<_>>(),
	})
}

async fn groups_json(state: &AppState, user_id: i64) -> ApiResult<Value> {
	let rows: Vec<GroupRow> = sqlx::query_as(
		"SELECT id, name, sort_order FROM filter_group WHERE user_id = ? ORDER BY sort_order ASC, id ASC",
	)
	.bind(user_id)
	.fetch_all(&state.pool)
	.await?;
	let mut out = Vec::with_capacity(rows.len());
	for row in &rows {
		out.push(group_json(row, &rules_of(&state.pool, row.id).await?));
	}
	Ok(json!(out))
}

/// The library query's groupId arm: the group must belong to the requesting user, same
/// scoping as categories
pub async fn rules_for_library(pool: &SqlitePool, user_id: i64, group_id: i64) -> ApiResult<Vec<StoredRule>> {
	group_row(pool, user_id, group_id).await?;
	rules_of(pool, group_id).await
}

/// One bind in compiled-fragment order: User is the request's user id, Id a rule value
pub enum GroupBind {
	User,
	Id(i64),
}

/// Compiles a group's rules into AND-ed WHERE fragments (with leading " AND " each) and
/// their ordered binds. Aidoku's filter loop: includes within one valued dimension OR,
/// excludes apply per value, dimensions and flag rules AND.
pub fn compile_group(rules: &[StoredRule]) -> ApiResult<(String, Vec<GroupBind>)> {
	let mut sql = String::new();
	let mut binds: Vec<GroupBind> = Vec::new();
	for rule in rules {
		match (rule.r#type.as_str(), rule.exclude) {
			// A finished download always has its chapter_user row by construction
			("downloaded", exclude) => push_flag(&mut sql, &mut binds, exclude, "cu.is_downloaded = 1"),
			("started", exclude) => push_flag(&mut sql, &mut binds, exclude, "cu.last_read_at IS NOT NULL"),
			("completed", false) => sql.push_str(" AND m.status = 2"),
			("completed", true) => sql.push_str(" AND m.status != 2"),
			_ => {}
		}
	}
	// has_unread is its own shape (no chapter_user join): a chapter with no row counts as unread
	for rule in rules.iter().filter(|r| r.r#type == "has_unread") {
		let head = if rule.exclude { " AND NOT EXISTS" } else { " AND EXISTS" };
		sql.push_str(&format!(
			"{head} (SELECT 1 FROM chapter c WHERE c.manga_id = m.id AND NOT EXISTS (SELECT 1 FROM chapter_user cu WHERE cu.chapter_id = c.id AND cu.user_id = ? AND cu.is_read = 1))"
		));
		binds.push(GroupBind::User);
	}
	push_valued(&mut sql, &mut binds, rules, "source", "m.source_id")?;
	push_valued(&mut sql, &mut binds, rules, "content_rating", "m.content_rating")?;
	push_category(&mut sql, &mut binds, rules)?;
	Ok((sql, binds))
}

fn push_flag(sql: &mut String, binds: &mut Vec<GroupBind>, exclude: bool, column: &str) {
	let head = if exclude { " AND NOT EXISTS" } else { " AND EXISTS" };
	sql.push_str(&format!(
		"{head} (SELECT 1 FROM chapter c JOIN chapter_user cu ON cu.chapter_id = c.id AND cu.user_id = ? WHERE c.manga_id = m.id AND {column})"
	));
	binds.push(GroupBind::User);
}

/// Rule values were validated as integers on write; an unparseable one means a hand-corrupted row
fn parse_values(rules: &[StoredRule], r#type: &str, exclude: bool) -> ApiResult<Vec<i64>> {
	rules
		.iter()
		.filter(|r| r.r#type == r#type && r.exclude == exclude)
		.map(|r| {
			r.value
				.as_deref()
				.and_then(|v| v.parse::<i64>().ok())
				.ok_or_else(|| ApiError::internal(format!("corrupt {type} rule value")))
		})
		.collect()
}

fn push_valued(sql: &mut String, binds: &mut Vec<GroupBind>, rules: &[StoredRule], r#type: &str, column: &str) -> ApiResult<()> {
	let includes = parse_values(rules, r#type, false)?;
	let excludes = parse_values(rules, r#type, true)?;
	if !includes.is_empty() {
		sql.push_str(&format!(
			" AND {column} IN ({})",
			vec!["?"; includes.len()].join(",")
		));
		binds.extend(includes.into_iter().map(GroupBind::Id));
	}
	if !excludes.is_empty() {
		sql.push_str(&format!(
			" AND {column} NOT IN ({})",
			vec!["?"; excludes.len()].join(",")
		));
		binds.extend(excludes.into_iter().map(GroupBind::Id));
	}
	Ok(())
}

fn push_category(sql: &mut String, binds: &mut Vec<GroupBind>, rules: &[StoredRule]) -> ApiResult<()> {
	let includes = parse_values(rules, "category", false)?;
	let excludes = parse_values(rules, "category", true)?;
	if !includes.is_empty() {
		sql.push_str(&format!(
			" AND EXISTS (SELECT 1 FROM category_manga cm WHERE cm.manga_id = m.id AND cm.user_id = ? AND cm.category_id IN ({}))",
			vec!["?"; includes.len()].join(",")
		));
		binds.push(GroupBind::User);
		binds.extend(includes.into_iter().map(GroupBind::Id));
	}
	if !excludes.is_empty() {
		sql.push_str(&format!(
			" AND NOT EXISTS (SELECT 1 FROM category_manga cm WHERE cm.manga_id = m.id AND cm.user_id = ? AND cm.category_id IN ({}))",
			vec!["?"; excludes.len()].join(",")
		));
		binds.push(GroupBind::User);
		binds.extend(excludes.into_iter().map(GroupBind::Id));
	}
	Ok(())
}

pub async fn list_groups(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
) -> ApiResult<Json<Value>> {
	Ok(Json(groups_json(&state, user.id).await?))
}

#[derive(Debug, Deserialize)]
pub struct CreateGroupRequest {
	pub name: String,
	#[serde(default)]
	pub rules: Vec<RuleInput>,
}

async fn ensure_unique_name(pool: &SqlitePool, user_id: i64, name: &str, except_id: Option<i64>) -> ApiResult<()> {
	let duplicate: Option<(i64,)> = match except_id {
		Some(id) => {
			sqlx::query_as("SELECT id FROM filter_group WHERE user_id = ? AND name = ? AND id != ?")
				.bind(user_id)
				.bind(name)
				.bind(id)
				.fetch_optional(pool)
				.await?
		}
		None => {
			sqlx::query_as("SELECT id FROM filter_group WHERE user_id = ? AND name = ?")
				.bind(user_id)
				.bind(name)
				.fetch_optional(pool)
				.await?
		}
	};
	if duplicate.is_some() {
		return Err(ApiError::duplicate("filter group name already exists"));
	}
	Ok(())
}

async fn insert_rules(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, group_id: i64, rules: &[Rule]) -> ApiResult<()> {
	for rule in rules {
		sqlx::query("INSERT INTO filter_group_rule (group_id, type, value, exclude) VALUES (?, ?, ?, ?)")
			.bind(group_id)
			.bind(&rule.r#type)
			.bind(rule.value.map(|v| v.to_string()))
			.bind(rule.exclude)
			.execute(&mut **tx)
			.await?;
	}
	Ok(())
}

pub async fn create_group(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Json(body): Json<CreateGroupRequest>,
) -> ApiResult<Json<Value>> {
	let name = body.name.trim();
	if name.is_empty() {
		return Err(ApiError::bad_request("name must not be empty"));
	}
	let rules = validate_rules(&body.rules)?;
	ensure_unique_name(&state.pool, user.id, name, None).await?;
	let (sort_order,): (i64,) =
		sqlx::query_as("SELECT COALESCE(MAX(sort_order) + 1, 0) FROM filter_group WHERE user_id = ?")
			.bind(user.id)
			.fetch_one(&state.pool)
			.await?;
	let mut tx = crate::db::begin_write(&state.pool).await?;
	sqlx::query("INSERT INTO filter_group (user_id, name, sort_order) VALUES (?, ?, ?)")
		.bind(user.id)
		.bind(name)
		.bind(sort_order)
		.execute(&mut *tx)
		.await?;
	let (id,): (i64,) = sqlx::query_as("SELECT id FROM filter_group WHERE user_id = ? AND name = ?")
		.bind(user.id)
		.bind(name)
		.fetch_one(&mut *tx)
		.await?;
	insert_rules(&mut tx, id, &rules).await?;
	tx.commit().await?;
	Ok(Json(group_json(
		&group_row(&state.pool, user.id, id).await?,
		&rules_of(&state.pool, id).await?,
	)))
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateGroupRequest {
	pub name: Option<String>,
	pub rules: Option<Vec<RuleInput>>,
}

/// Parses the body by hand: axum's Json rejection is 422, while the endpoint convention for invalid values is 400; an empty body is a no-op
pub async fn update_group(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(id): Path<i64>,
	body: Option<Json<Value>>,
) -> ApiResult<Json<Value>> {
	let row = group_row(&state.pool, user.id, id).await?;
	let patch = match body {
		None => UpdateGroupRequest::default(),
		Some(Json(value)) => serde_json::from_value(value)
			.map_err(|e| ApiError::bad_request(format!("invalid filter group patch: {e}")))?,
	};
	let name = match patch.name {
		Some(name) => {
			let name = name.trim();
			if name.is_empty() {
				return Err(ApiError::bad_request("name must not be empty"));
			}
			ensure_unique_name(&state.pool, user.id, name, Some(id)).await?;
			Some(name.to_string())
		}
		None => None,
	};
	let rules = patch.rules.as_deref().map(validate_rules).transpose()?;
	if name.is_none() && rules.is_none() {
		return Ok(Json(group_json(&row, &rules_of(&state.pool, id).await?)));
	}
	let mut tx = crate::db::begin_write(&state.pool).await?;
	if let Some(name) = &name {
		sqlx::query("UPDATE filter_group SET name = ? WHERE id = ? AND user_id = ?")
			.bind(name)
			.bind(id)
			.bind(user.id)
			.execute(&mut *tx)
			.await?;
	}
	if let Some(rules) = &rules {
		sqlx::query("DELETE FROM filter_group_rule WHERE group_id = ?")
			.bind(id)
			.execute(&mut *tx)
			.await?;
		insert_rules(&mut tx, id, rules).await?;
	}
	tx.commit().await?;
	Ok(Json(group_json(
		&group_row(&state.pool, user.id, id).await?,
		&rules_of(&state.pool, id).await?,
	)))
}

pub async fn delete_group(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
	group_row(&state.pool, user.id, id).await?;
	sqlx::query("DELETE FROM filter_group WHERE id = ? AND user_id = ?")
		.bind(id)
		.bind(user.id)
		.execute(&state.pool)
		.await?;
	Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
pub struct ReorderRequest {
	pub ids: Vec<i64>,
}

pub async fn reorder_groups(
	State(state): State<Arc<AppState>>,
	Extension(user): Extension<crate::auth::AuthUser>,
	Json(body): Json<ReorderRequest>,
) -> ApiResult<Json<Value>> {
	let mut tx = crate::db::begin_write(&state.pool).await?;
	for (index, id) in body.ids.iter().enumerate() {
		sqlx::query("UPDATE filter_group SET sort_order = ? WHERE id = ? AND user_id = ?")
			.bind(index as i64)
			.bind(id)
			.bind(user.id)
			.execute(&mut *tx)
			.await?;
	}
	tx.commit().await?;
	Ok(Json(groups_json(&state, user.id).await?))
}
