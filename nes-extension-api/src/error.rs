//! 扩展 ABI 的错误类型（中文 Display）。

use core::fmt;
use std::error::Error;

use crate::runtime::JsContextId;

/// 扩展执行面上的错误（引擎侧看到的全是这个形态 —— 不暴露任何第三方错误类型）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtError {
    /// JS 运行时（引擎进程）初始化失败。
    RuntimeInit(String),
    /// JS 上下文创建失败。
    ContextCreate(String),
    /// 上下文句柄不存在（已被丢弃或从未创建）。
    UnknownContext(JsContextId),
    /// 模块装载失败（语法错误 / 顶层求值异常）。
    Load(String),
    /// 调用失败（函数不存在 / 执行期异常 / 返回不可转换）。
    CallFailed(String),
    /// 值转换失败（NesValue <-> 引擎值的双向边界上）。
    Convert(String),
    /// 能力调用被拒绝（能力未接入 / 参数越界 / 目标不存在）。
    Capability(String),
}

impl fmt::Display for ExtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExtError::RuntimeInit(detail) => write!(f, "扩展运行时初始化失败：{detail}"),
            ExtError::ContextCreate(detail) => write!(f, "扩展上下文创建失败：{detail}"),
            ExtError::UnknownContext(id) => write!(f, "扩展上下文不存在：{}", id.0),
            ExtError::Load(detail) => write!(f, "扩展模块装载失败：{detail}"),
            ExtError::CallFailed(detail) => write!(f, "扩展调用失败：{detail}"),
            ExtError::Convert(detail) => write!(f, "扩展值转换失败：{detail}"),
            ExtError::Capability(detail) => write!(f, "扩展能力调用被拒绝：{detail}"),
        }
    }
}

impl Error for ExtError {}

#[cfg(test)]
mod tests {
    use super::ExtError;
    use crate::runtime::JsContextId;

    // 中文 Display：每变体至少一条可读文案（ASCII 测试字面量纪律只约束字面量，
    // 断言目标是中文文案的存在性，用包含关键字的前缀匹配）。
    #[test]
    fn display_is_human_readable_per_variant() {
        let cases = [
            (ExtError::RuntimeInit("x".into()), "初始化失败"),
            (ExtError::ContextCreate("x".into()), "创建失败"),
            (ExtError::UnknownContext(JsContextId(7)), "上下文不存在"),
            (ExtError::Load("x".into()), "装载失败"),
            (ExtError::CallFailed("x".into()), "调用失败"),
            (ExtError::Convert("x".into()), "转换失败"),
            (ExtError::Capability("x".into()), "能力调用被拒绝"),
        ];
        for (err, frag) in cases {
            let text = err.to_string();
            assert!(text.contains(frag), "{frag} missing in: {text}");
        }
    }
}
