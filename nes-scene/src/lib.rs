//! NES 2.0 场景树核心 —— M1 骨架 + M2 反射与场景序列化 + M3 资源注册表接入。
//!
//! 对应设计文档：`NES2.0_节点场景树_接口草案_v1.md`
//!
//! # 进度
//!
//! **M1（已完成）**：身份模型 + arena 存储 + 延迟结构变更 + 确定性遍历 + 变换脏传播。
//! 四条出口准则见 `tests/m1.rs`：遍历顺序稳定可复现；`Reparent` 拒绝成环；
//! 遍历中发起结构变更不破坏本次遍历；变换脏传播正确（改父影响子，改子不影响父）。
//!
//! **M2（已完成）**：属性反射 + 场景序列化 / 实例化 / 打包。出口准则见 `tests/m2.rs`：
//! 1. 任意节点的属性可被枚举、读、写、校验（[`schema`] / [`props`] / [`value`]）；
//! 2. 同一棵树序列化两次逐字节相同（确定性打包，可直接进版本控制）；
//! 3. 场景可回读实例化，且实例化结果与原始树在结构、属性、世界变换上等价；
//! 4. 版本高于引擎的场景被明确拒绝，引擎不认识扩展属性被原样保留（前向兼容）。
//!
//! **M3（本期）**：属性里的资源引用接入全局注册表（[`resources`]）。出口准则见
//! `tests/m3.rs`：
//! 1. 节点属性里的 `Value::Resource(n)` 经 [`ResourceTable`] 解析为真实
//!    [`AssetKey`] 并完成注册与加载，未声明的引用退化为悬垂项而不是报错；
//! 2. 资源文件在磁盘上被改写后，热重载经 [`ResourceTable::poll`] 把版本变化
//!    传播到对应槽位，[`ResourceTable::take_dirty`] 只给出真正变过的槽位；
//! 3. 依赖图与引用计数闭环：声明依赖 → 绑定 → 场景持有 → 释放 → 回收，
//!    成环依赖被明确拒绝；
//! 4. 资源声明随场景一起往返：写出 → 回读后，属性里的 `n` 仍指向同一路径/类别
//!    同一槽位，且空资源表不写出（M2 存量场景逐字节不变）。
//!
//! **未做**：渲染解引用（M4，把就绪字节变成 GPU 资源）、Scratch/JS 兼容层（M5）。
//!
//! # 对 v1 草案的实现层修订（实现反馈设计）
//!
//! 1. **世界变换缓存改用 [`Affine`] 而非 `Transform2D`。**
//!    草案第 3 节写 `world: Transform2D`。实现时发现：父子复合需要矩阵乘法，
//!    而把乘积矩阵还原回 (pos, rot, scale, skew) 是带有分解歧义的运算，
//!    在非均匀缩放的父节点下会产生误差累积，且旋绕 ±180° 附近不稳定。
//!    改为：`local` 保留人类可读的 `Transform2D`（面向编辑器与脚本），
//!    `world` 缓存精确的 `Affine`（面向渲染与查询）。语义不变，精度提升。
//!
//! 2. **新增 `Cmd::Spawn`，允许在回调中请求创建子节点。**
//!    草案第 6 节的 `queue(TreeOp)` 对 `Add` 需要调用方先持有一个 `NodeId`，
//!    但回调里只有只读树（`NodeCtx`），无法分配 arena 槽位。
//!    补一条命令变体解决，且它同样延迟到帧首落地，不破坏确定性。
//!
//! 3. **节点专有字段迁入属性表（M2）。**
//!    M1 的 `NodeKind::Sprite2D { texture }` 这类"变体带字段"在 M2 撞墙：
//!    专有字段没有统一读写口、与 schema 形成两份事实来源、且无法承载
//!    引擎不认识的扩展属性。M2 起 `NodeKind` 只表类型，一切设计时数据都在
//!    [`PropStore`] 里，由 [`NodeSchema`] 提供类型、默认值、校验与编辑器提示。
//!    详见 `node.rs` 的 `NodeKind` 文档。
//!
//! 4. **资源引用拆成"场景内槽位 / 项目内路径 / 运行时键"三层（M3）。**
//!    草案把 `Value::Resource(u64)` 直接当作注册表键。实现时发现那样等于把
//!    进程内 arena 的槽位与代际写进场景文件：换个会话读同一份场景，同一个
//!    数字可能指向完全不同的资源，进版本控制更无意义。
//!    改为：属性里存**场景内槽位** [`ResId`]（随场景序列化，稳定、可 diff），
//!    [`ResourceTable`] 负责 `ResId → (AssetPath, AssetKind) → AssetKey` 的解析，
//!    渲染侧只认 `RenderAssetKeyView`，于是场景层与渲染层看的是同一个身份的
//!    两个投影，而不是各自维护一套编号。`Resource(0)` 保留为"未绑定"，
//!    M2 的存量场景无需迁移即可读入（表现为悬垂项）。
//!
//! # 接口纪律（本 crate 最重要的不变式）
//!
//! 行为代码（脚本、将来的 JS 扩展）只能通过 [`NodeCtx`] 接触树：
//! 只读树 + 命令缓冲。它**在类型层面**就无法直接改树结构，
//! 因此树不变式（有序、双向一致、无环）不可能被外部代码破坏。

#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]

pub mod determinism;
pub mod identity;
pub mod node;
pub mod path;
pub mod props;
pub mod resources;
pub mod scene_io;
pub mod schema;
pub mod script;
pub mod transform;
pub mod tree;
pub mod value;

pub use identity::{Arena, NodeHandle, NodeId};
pub use node::{NodeKind, NodeKindTag};
pub use path::{NodePath, PathError, PathSeg};
pub use props::{PropDiff, PropError, PropStore};
pub use resources::{
    asset_kind_of_hint, AdoptReport, BindReport, ResEntry, ResId, ResourceTable, ResourceView,
    TableError,
};
pub use scene_io::{
    diff_instance_overrides, doc_to_ron, expand_subscenes, instantiate, instantiate_doc,
    instantiate_doc_with_resources, parse_ron, to_doc, to_doc_omitting_defaults, to_doc_with_resources,
    write_ron, write_ron_with_resources, InstanceOverride, NodeDoc, PackOptions, PackedScene,
    ParseError, ResourceDoc, SceneDoc, FORMAT_VERSION, PROP_SUB_SCENE,
};
pub use schema::{EditorHint, NodeSchema, PropDesc};
pub use script::{
    compile_script, Op, Script, ScriptEntry, ScriptVm, StackVal, HALT_LOCAL, INIT_LOCAL,
    SCRIPT_MAX_STEPS,
};
pub use transform::{Affine, Transform2D, Vec2};
pub use determinism::scene_fingerprint;
pub use tree::{
    Cmd, NoObserver, NodeCtx, NodeData, NodeFlags, Observers, ProcessMode, SceneObserver,
    SceneTree, Signal, SignalConnection, SignalConnectionId, SignalCtx, SignalFilter,
    SignalHandler, TickStats, TreeEvent, TreeOp, SIGNAL_DELIVERY_CAP,
};
pub use value::{Value, ValueType};
