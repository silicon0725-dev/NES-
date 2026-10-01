# NES 2.0 · S6.31 脚本进场景文件 v1

> 交付日期：2026-10-01　｜　状态：**Script 节点 `source` 属性 —— 场景文件自带行为（行为层闭环）**
> 前置：S6.19~30 脚本 VM 与文本语言全链。

---

## 0. 一句话结论

Script 节点新增 `source` 属性（Str，schema 注册）：**非空即由 ScriptVm
在 attach 时编译装载**——场景文件从此是"结构 + 资源引用 + **行为**"的
自包含单元，宿主零注册步骤。多行源码经 RON 字符串转义（`\n`）随场景
往返逐字不丢；与 `registry_key` **互斥**（一个节点一个事实来源）；
编译错带脚本文本行/列入 attach 缺口。出口准则 T-Cmp-31 + T-Script-R2
（磁盘场景 -> 加载 -> attach_all -> 信号 -> **像素**）全过。全仓测试
**160** / 34 / 42 / 40 / 83 / **28** 全绿，守卫 11/11，clippy 零警告。

---

## 1. 语义与裁决

### 1.1 双装载路径与互斥

| 路径 | 谁提供脚本 | 适用 |
|---|---|---|
| `source` 属性（本轮） | 场景文件内嵌文本，attach 时编译 | 自包含场景、编辑器保存 |
| `registry_key`（既有） | 宿主注册表预注册 | M5 兼容层（Scratch/JS 编译产物） |

两者同设是**接线错误**：一个节点一个事实来源，如实拒绝不猜（错误指名
互斥）。内嵌脚本的注册表键用节点派生键（`__inline__:{node}`），宿主
命名空间不被场景文件占用。

### 1.2 往返与转义

- RON 写出转义（`"` `\` `\n` `\r` `\t`）、读入反转义——**既有机制零
 改动**（S6.31 只是首次用多行串实质性地压它，T-Cmp-31 逐字断言）；
- schema 默认空串 + `omit_defaults` 写出省略——**存量场景文件逐字节
 不变**（老文件无 source 属性，照常走 registry_key）。

### 1.3 attach 缺口与编译错

内嵌脚本的编译错误（语法/保留字等）进入 attach 的 `Err`/`attach_all`
缺口清单，**携带脚本文本的行/列**（"内嵌脚本编译失败：{行:列:原因}"）
—— 修场景文件的人看到的行号是**脚本文本里的行号**，不是 RON 文件的。

### 1.4 运行时形态

`load_scene`（含子场景展开）→ `vm.attach_all(rt.tree_mut())`（source
属性自动编译装载）→ `frame_with(frame, &mut vm)`（VM 即观察者，process
入口驱动）+ 宿主发信号（信号入口驱动）。**场景文件换掉，行为就换掉**
—— 与纹理/子场景热重载同一"文件即事实"哲学（脚本热重载见遗留）。

## 2. 实现落点

| 位置 | 改动 |
|---|---|
| `schema.rs` Script | `source` 属性（Str，默认空；omit_defaults 存量不变） |
| `script.rs` attach | source 优先路径：互斥校验 → `compile_script`（错入缺口）→ 派生键注册；`require_key` 辅助抽出 |

nes-runtime 零改动（`attach_all` 既有签名收 source 自动生效）。

## 3. 出口准则

### 3.1 场景层（`s6_script_text.rs` 追加，1/1）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-Cmp-31 | ① source 装载（零注册）驱动像素；② 多行源码 RON 转义往返逐字不丢 + 回读树行为不丢（循环/++/后置计算）；③ 三错：互斥指名 / 编译错带行号 / 双空回 registry_key 错误 | ✅ |

### 3.2 运行时端到端（`criterion_script.rs` 追加，1/1）

| 编号 | 契约 | 结果 |
|---|---|---|
| T-Script-R2 | 手写磁盘场景（source 内嵌、RON 转义换行/引号）→ 加载 → attach_all 零注册 → 宿主只发信号 → **精灵像素两帧右移**；回存后 source 属性与转义在文件里 | ✅ |

## 4. 遗留与后续

| 事项 | 状态 |
|---|---|
| 脚本热重载（改 source 属性 / poll 后重 attach —— 复编译重挂载） | 未启动（机制已备：属性写 + attach 幂等替换） |
| 外置脚本文件（`.nes` 资产 + registry_key 引用——多场景共享脚本） | 未启动（与纹理/子场景的资产路径同构） |
| 编辑器脚本面板（source 属性的富文本编辑 + 行号对齐编译错） | 未启动 |
| headless Linux / WM_SIZE / DPI | 沿各文档遗留表 |

## 5. 记账

- 测试基线：**nes-scene 160**（159 -> 160，+T-Cmp-31）/ nes-asset 34 /
  nes-render-api 40 / nes-render-extract 42 / nes-render-wgpu 83 /
  **nes-runtime 28**（27 -> 28，+T-Script-R2）—— 全绿；
- 守卫 G1~G11 **11/11**；六 crate `clippy --all-targets` 零警告；
- 改动面：nes-scene（schema 一属性 + attach 一路径）与测试；
  nes-runtime 零改动（新测试除外）。

*（内容由AI生成，仅供参考）*
