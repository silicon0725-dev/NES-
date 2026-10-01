# NES 2.0 · S6.33 外置 .nes 脚本资产 v1

> 交付日期：2026-10-01　｜　状态：**`script` 属性（资源槽位 -> .nes 文件）—— 脚本成为一等资产**
> 前置：S6.31/32 内嵌脚本与热重载；纹理/子场景的资产路径模式（M3/S6.6）。

---

## 0. 一句话结论

Script 节点第三路挂载：`script` 属性（**Resource 槽位**，与纹理
`texture`、包装 `sub_scene` 同构）指向场景资源表 `kind: "Script"` 条目
（`.nes` 纯文本文件）。[`attach_all_with_sources`]（表 + 注入的文件读取器
—— VM 不碰文件系统，与 `expand_subscenes` 同一注入模式）解析装载；
[`poll_reloads_with_sources`] 外置热重载（**改 .nes 文件 -> 重读比对 ->
重编译 -> 下一帧新像素，不整树重载**）。三路挂载（source/script/
registry_key）**恰一非空**。同文件多节点**共享编译产物**（派生键
`__file__:{path}`）。出口准则 T-VM-09..10 + T-Script-R4 全过。全仓测试
**165** / 34 / 42 / 40 / 83 / **30** 全绿，守卫 11/11，clippy 零警告。

---

## 1. 语义与裁决

### 1.1 三路挂载（S6.33 收紧为表）

| 挂载 | 属性 | 谁提供脚本 | 适用 |
|---|---|---|---|
| 内嵌 | `source`（Str） | 节点自身文本 | 自包含场景（S6.31） |
| **外置**（本轮） | `script`（Resource 槽位） | `.nes` 文件（资源表声明） | **多场景共享**、编辑器外置编辑 |
| 注册表 | `registry_key`（Str） | 宿主 VM 注册表 | M5 兼容层（编译产物） |

**恰一非空**：多于一路指名全部违规路（"同时设了 source、script"）。
外置声明缺失（槽位不在表/非 Script 类/无路径）如实报错。

### 1.2 资产身份（与纹理/子场景同构）

- RON `resources` 段声明一次，多节点引用同一槽位；
- `bind_assets` 照常加载（字节 + 内容戳入注册表——Script 类资产从此
  参与资产管线的内容戳体系）；
- `save_scene` 往返：`script: Resource(n)` 与 `texture` 同一序列化路径；
- 同文件多节点**共享编译产物**（键 `__file__:{path}`，scripts 表自然
  去重）；但**各自安装**（处理器/连接按节点 —— 同文件双节点 = 信号
  命中两次，T-VM-10 钉死）。

### 1.3 外置热重载（与 S6.7 Scene 资产流的分工）

| 流 | 轮询方法 | 重建粒度 |
|---|---|---|
| Script 资产变化 | `vm.poll_reloads_with_sources` | **单节点重编译重挂载**（精灵位置等运行时状态保留） |
| Scene 资产变化 | `rt.poll_scene_reload`（S6.7） | **整树重载**（结构变化才需要） |

外置语义同 S6.32 内嵌：**戳 = 上次成功编译的文件文本**——读失败/编译
失败保留旧行为、错误进清单、修好下次 poll 即生效（last-good）。

### 1.4 注入读取器（VM 不碰文件系统）

`read: &mut dyn FnMut(&str) -> Result<String, String>` —— 与子场景展开的
`expand_subscenes(doc, load)` 同一模式：场景 crate 保持纯（测试用内存
映射，运行时用磁盘闭包）。旧 `attach_all`/`poll_reloads` 对外置节点
**如实报"需要 with_sources"**（不静默哑挂）；`attach` 单点对外置同样报。

## 2. 实现落点

| 位置 | 改动 |
|---|---|
| `schema.rs` Script | `script` 属性（Resource，hint kind "script"） |
| `resources.rs` | 提示映射 `.nes` -> Script |
| `script.rs` | `check_exclusive`（三路恰一）/ `install`（尾段共享抽取）/ `attach_external`（`__file__:{path}` 键 + file_stamp）/ `attach_all_with_sources` / `poll_reloads_with_sources` / `external_path`（槽位->路径，类别校验） |
| `ScriptVm` 字段 | `file_stamp`（外置 last-good 戳） |

nes-runtime 零改动（宿主注入闭包 + 表快照）。

## 3. 出口准则

### 3.1 场景层（`s6_script.rs` 追加，2/2）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-VM-09 | 外置装载驱动；三路互斥三例（双路/三路指名）；文件消失读错指名 + **旧行为保留**；旧 attach_all 对外置报需 sources | ✅ |
| T-VM-10 | 同文件双节点：共享（同信号双命中）、坏文本双进清单 + 旧行为保留、修好双重载、未变不重载 | ✅ |

### 3.2 运行时端到端（`criterion_script.rs` 追加，1/1）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-Script-R4 | 磁盘场景（资源表 kind:Script + `script: Resource(2)`）→ 加载（2 资产入注册表）→ attach_all_with_sources（磁盘闭包）→ 信号 → 像素 +16；**改 .nes 文件 → poll_reloads_with_sources → 新像素 -8（不整树重载）**；save 往返槽位引用 | ✅ |

## 4. 遗留与后续

| 事项 | 状态 |
|---|---|
| 注册表内容戳驱动的自动重载（rt.poll_reloads 已能看到 Script 资产变化，接线到 vm 轮询属宿主惯用法） | 文档口径（宿主串两 poll） |
| `.nes` 文件的语法提示/编辑器支持 | 未启动 |
| headless Linux / WM_SIZE / DPI | 沿各文档遗留表 |

## 5. 记账

- 测试基线：**nes-scene 165**（163 -> 165，+T-VM-09..10）/ nes-asset 34 /
  nes-render-api 40 / nes-render-extract 42 / nes-render-wgpu 83 /
  **nes-runtime 30**（29 -> 30，+T-Script-R4）—— 全绿；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
- 改动面：nes-scene（schema/resources/script.rs）与测试；nes-runtime 零改动。

*（内容由AI生成，仅供参考）*
