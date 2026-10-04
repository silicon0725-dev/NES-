//! NesValue <-> JS 值的双向转换（值边界的唯一通道）。
//!
//! 口径：
//! * `undefined` 与 `null` 归一为 [`NesValue::Null`]（JS 侧二义在此消失）；
//! * JS 数值唯一形态是 double，整数字面量也落 [`NesValue::F64`]；
//! * 函数与宿主对象**不跨越边界**（读作 [`NesValue::Null`]，不报错 ——
//!   扩展把函数塞进返回值是脚本侧 bug，不值得炸帧）；
//! * 对象转换保序（键遍历序即存储序 —— 确定性口径）。

use rquickjs::{Array, Ctx, Function, IntoJs, Object, Value};

use nes_extension_api::{ExtError, NesValue};

/// [`NesValue`] -> JS 值。
pub fn nes_to_js<'js>(ctx: &Ctx<'js>, v: &NesValue) -> Result<Value<'js>, rquickjs::Error> {
    match v {
        NesValue::Null => Ok(Value::new_null(ctx.clone())),
        NesValue::Bool(b) => (*b).into_js(ctx),
        NesValue::F64(x) => (*x).into_js(ctx),
        NesValue::Str(s) => s.as_str().into_js(ctx),
        NesValue::Array(items) => {
            let arr = Array::new(ctx.clone())?;
            for (i, item) in items.iter().enumerate() {
                arr.set(i, nes_to_js(ctx, item)?)?;
            }
            arr.into_js(ctx)
        }
        NesValue::Object(pairs) => {
            let obj = Object::new(ctx.clone())?;
            for (k, val) in pairs {
                obj.set(k.as_str(), nes_to_js(ctx, val)?)?;
            }
            obj.into_js(ctx)
        }
    }
}

/// JS 值 -> [`NesValue`]（转换失败走 [`ExtError::Convert`] 语义的映射错误）。
pub fn js_to_nes(v: &Value<'_>) -> Result<NesValue, rquickjs::Error> {
    if v.is_undefined() || v.is_null() {
        return Ok(NesValue::Null);
    }
    // 函数先于对象判定（JS 里函数也是对象）—— 不进值边界（读作 Null）。
    if v.is_function() {
        return Ok(NesValue::Null);
    }
    if let Some(b) = v.as_bool() {
        return Ok(NesValue::Bool(b));
    }
    if let Some(x) = v.as_number() {
        return Ok(NesValue::F64(x));
    }
    if let Some(s) = v.as_string() {
        return Ok(NesValue::Str(s.to_string()?));
    }
    if let Some(arr) = v.as_array() {
        let len = arr.len();
        let mut items = Vec::with_capacity(len);
        for i in 0..len {
            let item: Value = arr.get(i)?;
            items.push(js_to_nes(&item)?);
        }
        return Ok(NesValue::Array(items));
    }
    if let Some(obj) = v.as_object() {
        let mut pairs = Vec::new();
        for key in obj.keys::<String>() {
            let key = key?;
            let val: Value = obj.get(key.as_str())?;
            pairs.push((key, js_to_nes(&val)?));
        }
        return Ok(NesValue::Object(pairs));
    }
    // 其余（symbol / bigint / 外部等）：P0 一律 Null，不炸帧。
    Ok(NesValue::Null)
}

/// 便捷：按名取全局函数（不存在时报 `ExtError::CallFailed`）。
pub fn global_function<'js>(
    ctx: &Ctx<'js>,
    name: &str,
) -> Result<Function<'js>, ExtError> {
    ctx.globals()
        .get(name)
        .map_err(|_| ExtError::CallFailed(format!("global function not found: {name}")))
}

#[cfg(test)]
mod tests {
    use super::{global_function, js_to_nes, nes_to_js};
    use nes_extension_api::NesValue;
    use rquickjs::{Context, Runtime};

    fn with_ctx(f: impl FnOnce(&rquickjs::Ctx<'_>)) {
        let rt = Runtime::new().unwrap();
        let ctx = Context::full(&rt).unwrap();
        ctx.with(|ctx| f(&ctx));
    }

    fn rt() -> (Runtime, Context) {
        let rt = Runtime::new().unwrap();
        let ctx = Context::full(&rt).unwrap();
        (rt, ctx)
    }

    #[test]
    fn every_variant_survives_the_round_trip() {
        let (rt, ctx) = rt();
        ctx.with(|ctx| {
            ctx.eval::<(), _>(
                "function echo(x) { return x; }
                 function mkNull() { return null; }
                 function mkUndef() { return undefined; }
                 function mkBool() { return true; }
                 function mkInt() { return 42; }
                 function mkFloat() { return 1.5; }
                 function mkStr() { return \"txt\"; }
                 function mkArr() { return [1, \"a\", null]; }
                 function mkObj() { return { x: 1, y: \"z\", n: null }; }",
            )
            .unwrap();

            let cases: Vec<NesValue> = vec![
                NesValue::Null,
                NesValue::Bool(true),
                NesValue::F64(42.0),
                NesValue::F64(1.5),
                NesValue::str("txt"),
                NesValue::arr([NesValue::F64(1.0), NesValue::str("a"), NesValue::Null]),
                NesValue::obj([
                    ("x", NesValue::F64(1.0)),
                    ("y", NesValue::str("z")),
                    ("n", NesValue::Null),
                ]),
            ];
            for v in &cases {
                let js = nes_to_js(&ctx, v).unwrap();
                let back = js_to_nes(&js).unwrap();
                assert_eq!(&back, v, "round trip failed for {v:?}");
            }

            // JS 侧造值 -> Rust 值（每变体的另一半）。
            for (fn_name, expect) in [
                ("mkNull", NesValue::Null),
                ("mkUndef", NesValue::Null),
                ("mkBool", NesValue::Bool(true)),
                ("mkInt", NesValue::F64(42.0)),
                ("mkFloat", NesValue::F64(1.5)),
                ("mkStr", NesValue::str("txt")),
                (
                    "mkArr",
                    NesValue::arr([NesValue::F64(1.0), NesValue::str("a"), NesValue::Null]),
                ),
                (
                    "mkObj",
                    NesValue::obj([
                        ("x", NesValue::F64(1.0)),
                        ("y", NesValue::str("z")),
                        ("n", NesValue::Null),
                    ]),
                ),
            ] {
                let func = global_function(&ctx, fn_name).unwrap();
                let ret: rquickjs::Value = func.call(()).unwrap();
                assert_eq!(js_to_nes(&ret).unwrap(), expect, "{fn_name}");
            }
        });
        drop(rt);
    }

    #[test]
    fn function_values_do_not_cross_the_boundary() {
        with_ctx(|ctx| {
            ctx.eval::<(), _>("function f() { return function () {}; }").unwrap();
            let func = global_function(ctx, "f").unwrap();
            let ret: rquickjs::Value = func.call(()).unwrap();
            assert_eq!(js_to_nes(&ret).unwrap(), NesValue::Null);
        });
    }
}
