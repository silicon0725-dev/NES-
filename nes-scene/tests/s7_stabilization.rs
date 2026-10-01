//! S7 引擎收束（stabilization）契约：不横向加功能，用现有行为面反向
//! 审出的不变量钉死。
//!
//! | 编号 | 契约 |
//! |---|---|
//! | T-Stab-01 | 死节点清理**五表统一**：树整体替换后再经任一装载入口 -> `tracked_nodes()` 归零（S6.33 的 file_stamp 曾漏进清理表、attach_all_with_sources 曾整入口无清理） |

use nes_asset::AssetKind;
use nes_scene::{NodeKind, ResourceTable, ScriptVm, SceneTree, Transform2D, Value};

/// T-Stab-01：树 A 带外置脚本节点（script 槽位 -> .nes 文本）装载成功；
/// 换一棵全新的树 B（无任何脚本节点）再 `attach_all_with_sources` ——
/// 旧 NodeId 在 arena 查无，**五张登记表**（states/process_scripts/
/// inline_stamp/file_stamp/node_conn）必须全部剪空。
/// NodeId 带代号使陈旧键不致误交付，但清理是卫生不变量（防陈旧戳
/// 误判 + 内存不随整树重载次数增长）—— 收束阶段补齐口径并钉死。
#[test]
fn t_stab_01_dead_node_prune_unified_five_tables() {
    let mut t = SceneTree::new("root");
    let root = t.root();
    let sp = t.add_node(root, "sp", NodeKind::Sprite2D);
    t.set_local(sp, Transform2D::from_pos(0.0, 0.0));
    let brain = t.add_node(root, "brain", NodeKind::Script);
    t.apply_pending();

    // 资源表：kind:Script 槽位 -> brain.nes（内存读取器注入，VM 不碰文件系统）。
    let mut table = ResourceTable::new();
    let res = table.declare("Scripts/brain.nes", AssetKind::Script).unwrap();
    t.set_prop(brain, "script", Value::Resource(res.get() as u64))
        .unwrap();
    let files: std::cell::RefCell<std::collections::BTreeMap<String, String>> =
        Default::default();
    files
        .borrow_mut()
        .insert("Scripts/brain.nes".into(), "on \"step\" { sp.pos += (1.0, 0.0) }".into());
    let mut read = |path: &str| -> Result<String, String> {
        files
            .borrow()
            .get(path)
            .cloned()
            .ok_or_else(|| format!("文件不存在 {path}"))
    };

    let mut vm = ScriptVm::new();
    let issues = vm.attach_all_with_sources(&mut t, &table, &mut read);
    assert!(issues.is_empty(), "装载：{issues:?}");
    assert!(vm.tracked_nodes() >= 1, "装载后有登记");

    // 整树替换：全新树（无脚本节点）再装载 —— 旧登记必须全部剪掉。
    let mut t2 = SceneTree::new("fresh");
    let issues2 = vm.attach_all_with_sources(&mut t2, &table, &mut read);
    assert!(issues2.is_empty());
    assert_eq!(
        vm.tracked_nodes(),
        0,
        "五表全剪（states/process_scripts/inline_stamp/file_stamp/node_conn）"
    );

    // 内嵌路径同口径：装载 -> 整树替换 -> attach_all 归零。
    let mut t3 = SceneTree::new("root3");
    let b3 = t3.add_node(t3.root(), "b", NodeKind::Script);
    t3.set_prop(b3, "source", Value::Str("every { this.pos = (0.0, 0.0) }".into()))
        .unwrap();
    t3.apply_pending();
    assert!(vm.attach_all(&mut t3).is_empty());
    assert!(vm.tracked_nodes() >= 1);
    let mut t4 = SceneTree::new("fresh4");
    assert!(vm.attach_all(&mut t4).is_empty());
    assert_eq!(vm.tracked_nodes(), 0, "attach_all 路径同口径");
}
