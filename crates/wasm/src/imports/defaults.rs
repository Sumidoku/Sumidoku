use crate::{
	WasmEnv,
	runtime::store::{DefaultKind, DefaultValue, FFIResult, Ptr, StoreItem},
};
use wasmer::FunctionEnvMut;

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

pub fn get(mut env: FunctionEnvMut<WasmEnv>, key_ptr: Ptr, len: u32) -> FFIResult {
	let Ok(key) = env.data().read_string(&env, key_ptr, len) else {
		return Result::InvalidKey.into();
	};
	let Some(value) = env.data().defaults.get(&key).cloned() else {
		return Result::InvalidValue.into();
	};
	match value {
		DefaultValue::Data(data) => env.data_mut().store.store(StoreItem::Encoded(data)),
		DefaultValue::Bool(v) => env.data_mut().store.store_encoded(&v).unwrap_or(Result::FailedEncoding.into()),
		DefaultValue::Int(v) => env.data_mut().store.store_encoded(&v).unwrap_or(Result::FailedEncoding.into()),
		DefaultValue::Float(v) => env.data_mut().store.store_encoded(&v).unwrap_or(Result::FailedEncoding.into()),
		DefaultValue::String(v) => env.data_mut().store.store_encoded(&v).unwrap_or(Result::FailedEncoding.into()),
		DefaultValue::StringArray(v) => env.data_mut().store.store_encoded(&v).unwrap_or(Result::FailedEncoding.into()),
		DefaultValue::Null => Result::InvalidValue.into(),
	}
}

pub fn set(
	mut env: FunctionEnvMut<WasmEnv>,
	key_ptr: Ptr,
	len: u32,
	kind: u8,
	value_ptr: Ptr,
) -> FFIResult {
	let Ok(key) = env.data().read_string(&env, key_ptr, len) else {
		return Result::InvalidKey.into();
	};
	if kind > 6 {
		return Result::InvalidValue.into();
	}
	let Ok(data) = env.data().read_item_bytes(&env, value_ptr) else {
		return Result::FailedDecoding.into();
	};
	let value = match DefaultKind::from(kind) {
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
	env.data_mut().defaults.set(key, value);
	Result::Success.into()
}
