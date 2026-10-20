use crate::runtime::{
	env::Ctx,
	store::{DefaultKind, DefaultValue, FFIResult, Ptr, StoreItem},
};

enum Result {
	Success,
	InvalidKey,
	InvalidValue,
	FailedEncoding,
	FailedDecoding,
}

impl From<Result> for i32 {
	fn from(result: Result) -> Self {
		match result {
			Result::Success => 0,
			Result::InvalidKey => -1,
			Result::InvalidValue => -2,
			Result::FailedEncoding => -3,
			Result::FailedDecoding => -4,
		}
	}
}

pub fn get(ctx: &mut impl Ctx, key_ptr: Ptr, len: u32) -> FFIResult {
	let Ok(key) = ctx.read_string(key_ptr, len) else {
		return Result::InvalidKey.into();
	};
	let Some(value) = ctx.host().defaults.get(&key).cloned() else {
		return Result::InvalidValue.into();
	};
	match value {
		DefaultValue::Data(data) => ctx.host().store.store(StoreItem::Encoded(data)),
		DefaultValue::Bool(v) => ctx.host().store.store_encoded(&v).unwrap_or(Result::FailedEncoding.into()),
		DefaultValue::Int(v) => ctx.host().store.store_encoded(&v).unwrap_or(Result::FailedEncoding.into()),
		DefaultValue::Float(v) => ctx.host().store.store_encoded(&v).unwrap_or(Result::FailedEncoding.into()),
		DefaultValue::String(v) => ctx.host().store.store_encoded(&v).unwrap_or(Result::FailedEncoding.into()),
		DefaultValue::StringArray(v) => ctx.host().store.store_encoded(&v).unwrap_or(Result::FailedEncoding.into()),
		DefaultValue::Null => Result::InvalidValue.into(),
	}
}

pub fn set(
	ctx: &mut impl Ctx,
	key_ptr: Ptr,
	len: u32,
	kind: i32,
	value_ptr: Ptr,
) -> FFIResult {
	let Ok(key) = ctx.read_string(key_ptr, len) else {
		return Result::InvalidKey.into();
	};
	// wasmi host 参数没有 u8，kind 的 0..=6 校验在函数体内
	if !(0..=6).contains(&kind) {
		return Result::InvalidValue.into();
	}
	let Ok(data) = ctx.read_item_bytes(value_ptr) else {
		return Result::FailedDecoding.into();
	};
	let value = match DefaultKind::from(kind as u8) {
		DefaultKind::Data => Ok(DefaultValue::Data(data)),
		DefaultKind::Bool => postcard::from_bytes(&data).map(DefaultValue::Bool),
		DefaultKind::Int => postcard::from_bytes(&data).map(DefaultValue::Int),
		DefaultKind::Float => postcard::from_bytes(&data).map(DefaultValue::Float),
		DefaultKind::String => postcard::from_bytes(&data).map(DefaultValue::String),
		DefaultKind::StringArray => postcard::from_bytes(&data).map(DefaultValue::StringArray),
		DefaultKind::Null => Ok(DefaultValue::Null),
	};
	let Ok(value) = value else {
		return Result::FailedDecoding.into();
	};
	ctx.host().defaults.set(key, value);
	Result::Success.into()
}
