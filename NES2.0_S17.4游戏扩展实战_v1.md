# NES 2.0 — S17.4 游戏扩展实战（真游戏宿主装载真扩展）v1

## §0 结论

扩展生态第一次**实战闭环**：两个样例游戏（Dodge / Mini Dungeon）与编辑器壳层
按同一惯例装载资产根 `Extensions/*.js`，并接入一个**跨游戏通用的真扩展**
`shake.js`（近身警报相机震动）——扩展经既有 `load_extension_file` +
`update_extensions` 面（S17 第 1/2/3 期产物）驱动真实游戏状态，零引擎改动、
零新依赖。自动化验证（`nes-runtime/tests/s17_5_game_ext.rs`，T-GE-01..03）
钉住：装载计数、零诊断零故障、**敌人 AI 自然逼近触发相机震动**（Dodge 帧 54
首触，最大偏移 ≈3.9px）、震后相机**逐位归位**、同一份文件在 Mini Dungeon
同样触发、坏扩展不挡游戏。门禁全绿：十 crate `cargo test --release` **731**
passed（基线 728 + 新增 3）、clippy 0 ×10、守卫 15/15、两游戏 300 帧冒烟 +
编辑器 120 帧干净冒烟 + `NES_EDIT_DEMO=1` 420 帧全断言冒烟。场景文件与
regression/ 基线零改动（headless CLI 路径不装载扩展，基线逐位不动）。

## §1 宿主惯例（Extensions/ 目录 + 装载/更新接线）

### 1.1 惯例定义

* **位置**：`<资产根>/Extensions/*.js`——与 `Textures/`、`Audio/`、`Scripts/`
  同级的资产目录；文件即扩展，字典序 = 确定装载序。
* **装载时机**：装配后（脚本 attach + `mount_input_view` 之后）、帧循环之前，
  启动时**全装载**。装载失败（读盘失败 / JS 语法错误 / 顶层求值异常）只打
  日志继续——**一个坏扩展不挡游戏**（S17.1 隔离纪律的宿主半边：装载期失败 =
  该扩展缺席，不进帧；其余扩展与游戏照常）。
* **更新时机**：帧循环 simulate 之后调 `rt.update_extensions()`（S17 帧序
  契约：扩展看到当 tick 后状态；写队列当帧落地、下一帧呈现）。返回的诊断
  逐行 `println`——扩展错误在游戏控制台可见，不静默。

### 1.2 接线点（三个宿主逐一列明）

| 宿主 | 装载点 | 更新点 | 装载结果面 |
|---|---|---|---|
| `first_game.rs`（Dodge） | `mount_input_view` 之后、帧循环前，`load_extensions(&mut rt, &root)` | 每帧 `frame_windowed_with` 之后，诊断逐行 println | println（`[扩展] 已装载 <id> <- <路径>` / 失败行） |
| `dungeon_game.rs`（Mini Dungeon） | 同上（同一函数体，示例自包含） | 同上 | 同上 |
| `editor_shell.rs`（编辑器） | 初始树装配后、帧循环前（`editor_log` 建好之后） | 每帧 `frame_windowed_with` 之后，**仅 `play.playing` 时**推进 | println + Output dock 记 `ext loaded <id>` / `ext load failed: <名>` |

**编辑器裁决**：扩展的编辑器生态面 = **play-in-editor**——update 只在运行态
推进；编辑态不推进（扩展写树属运行期改动，编辑期写会绕过事务污染编辑会话，
与 S12-9 "运行期改动就是真改" 的边界一致）。装载仍在启动时一次完成（无卸载
API，重复 PLAY 不重复装载——`ExtensionManager` 挂在 `NesRuntime` 上，编辑器
本就没有自己的扩展逻辑，本期新增，无合并问题）。实测：编辑器演示场景
（obj1/obj2/obj3/cam）装载 hello.js + shake.js 后，play 态 hello.js 驱动
obj1 转圈、shake.js 因找不到 `player`/`e1..e3` 每帧空转（find 返回 null 跳过
——**跨场景无害性**的正面证据），编辑态两者都不推进。

### 1.3 零改动面（纪律核对）

* `first_game.ron` / `dungeon.ron` / `farm.ron` 场景文件：零改动（git status
  仅 3 个 example + 2 个新文件）。
* `examples/regression/dodge/`（expected_hash / scene / trace）：零改动；
  `t_abi_01_dodge_baseline`、`t_gp_01_dodge_gameplay_and_determinism` 原样绿
  ——headless CLI（`nes.exe`）与 `run_headless` 不装载扩展，基线路径逐位不动。
* 引擎（nes-runtime/src/extension.rs 等）：零改动——本期纯消费 S17 既有面。

## §2 真扩展 shake.js

### 2.1 设计

* **功能**：近距离警报相机震动——每帧读 player 与各敌人的最小距离，< 48px
  触发一次 20 次迭代的相机震动（每次写入基准位 + 随机小偏移），结束后归位；
  并 `nes.audio.play` 试播一发声效（混音器无此键 = 引擎静默丢弃；权限拒绝 =
  try/catch 吞掉——游戏默认无声资产，音频可选）。
* **通用性**：CONFIG 块声明节点名（`player` / `["e1","e2","e3"]` / `cam`），
  Dodge 与 Mini Dungeon 两场景同名兼容；节点不存在 = `nes.scene.find` 返回
  null 跳过（单敌缺席不影响其余敌人的距离计算）。
* **生成器震动（S17.3 C4 实战）**：`nes.onUpdate` 注册**一个**生成器协程 =
  无限监视循环（显式 `while (true)`，S17.3 裁决的静态可读形态）。`yield 1`
  按协程契约 = 停一帧，故监视每 2 帧采样一次、震动 20 次迭代跨 ~40 引擎帧
  （≈0.67s @60Hz）——如实写明，不谎称"每帧一写"。
* **滞回**：触发即 `armed = false`；震动期间协程体在 shake 循环内，监视代码
  根本不跑（结构性不重触发）；震后距离仍 < 64px 保持解除状态，**距离恢复
  > 64px（滞回带 48..64）才重新武装**——持续的威胁不会每帧重触发。
* **归位基准**：扩展启动后首次 `getPos(cam)` 记录基准位（两游戏 cam 初始都
  是 (192,108)，但扩展不写死——基准从场景实读）。所有偏移 = 基准 + jitter，
  结束 `setPos` 回基准**逐位**（T-GE-01/02 断言 dev == 0.0）。已知限制（P0
  如实）：若游戏脚本在震动期间移动相机，扩展会以陈旧基准归位——两样例游戏
  无 cam 脚本，不触发；通用化归 §5 遗留。
* **随机源**：`Math.random`（P0 不禁用但文档警告——确定性指纹场景下沙箱
  分区是后续期次的事；本扩展不进任何指纹基线路径）。

### 2.2 全文（`nes-runtime/examples/assets/Extensions/shake.js`，ASCII）

```js
// NES 2.0 S17.4 real extension: proximity alarm camera shake.
// ASCII only, per repo discipline. Cross-game by CONFIG: the node names
// below match BOTH sample games (Dodge and Mini Dungeon ship with nodes
// "player" / "e1".."e3" / "cam"); missing nodes make nes.scene.find
// return null and every pass is a no-op, so the extension also loads
// harmlessly into scenes without them (e.g. the editor demo scene).
//
// Behavior:
//   - nes.onUpdate registers ONE generator coroutine (S17.3 C4): an
//     infinite monitor loop, advanced by the host frame driver.
//   - Each pass reads the tick-end snapshot and computes the minimum
//     player-to-enemy distance. While armed, min distance < triggerDist
//     fires the alarm ONCE: a generator shake of `duration` iterations
//     (each iteration writes a random small offset around the recorded
//     camera base via nes.node.setPos, then `yield 1` = one paused frame
//     per the coroutine contract), then restores the base exactly.
//   - Hysteresis: the alarm re-arms only after distance recovers above
//     rearmDist, so a lingering threat cannot retrigger every frame.
//   - Camera base is captured once (first getPos of the cam node) and
//     never mutated by the shake itself: every offset is base + jitter
//     and the restore lands exactly on base (games may start the camera
//     anywhere; nothing here assumes 192,108).
//   - Optional audio: best-effort nes.audio.play of a CONFIG key. If the
//     mixer has no such key registered the engine drops it silently;
//     a permission denial is caught and swallowed (games ship without
//     audio assets -- sound is optional by design).
//
// Contract used (P0 frozen surface + S17.3 coroutine semantics):
//   nes.registerExtension(id)       announce this extension
//   nes.onUpdate(fn)                fn may be a generator function
//   nes.scene.find(name)            -> node ref (opaque number) or null
//   nes.node.getPos(ref)            -> [x, y] (tick-end snapshot)
//   nes.node.setPos(ref, x, y)      queued write, lands same frame
//   nes.audio.play(key, volume)     silent drop on unregistered key
nes.registerExtension("shake");

var CONFIG = {
  player: "player",
  enemies: ["e1", "e2", "e3"],
  cam: "cam",
  triggerDist: 48,
  rearmDist: 64,
  duration: 20,
  magnitude: 3,
  sound: "Audio/beep",
  volume: 0.5
};

nes.onUpdate(function* () {
  var base = null;
  var armed = true;
  while (true) {
    var pref = nes.scene.find(CONFIG.player);
    var cref = nes.scene.find(CONFIG.cam);
    if (pref === null || cref === null) {
      yield 1;
      continue;
    }
    if (base === null) {
      base = nes.node.getPos(cref);
      if (base === null) {
        yield 1;
        continue;
      }
    }
    var pp = nes.node.getPos(pref);
    var minD = null;
    if (pp !== null) {
      for (var i = 0; i < CONFIG.enemies.length; i++) {
        var eref = nes.scene.find(CONFIG.enemies[i]);
        if (eref === null) {
          continue;
        }
        var ep = nes.node.getPos(eref);
        if (ep === null) {
          continue;
        }
        var dx = ep[0] - pp[0];
        var dy = ep[1] - pp[1];
        var d = Math.sqrt(dx * dx + dy * dy);
        if (minD === null || d < minD) {
          minD = d;
        }
      }
    }
    if (minD === null) {
      yield 1;
      continue;
    }
    if (armed && minD < CONFIG.triggerDist) {
      armed = false;
      try {
        nes.audio.play(CONFIG.sound, CONFIG.volume);
      } catch (e) {
        // audio capability not granted in this host: stay silent.
      }
      for (var s = 0; s < CONFIG.duration; s++) {
        var ox = (Math.random() * 2.0 - 1.0) * CONFIG.magnitude;
        var oy = (Math.random() * 2.0 - 1.0) * CONFIG.magnitude;
        nes.node.setPos(cref, base[0] + ox, base[1] + oy);
        yield 1;
      }
      nes.node.setPos(cref, base[0], base[1]);
    } else if (!armed && minD > CONFIG.rearmDist) {
      armed = true;
    }
    yield 1;
  }
});
```

## §3 自动化验证（`nes-runtime/tests/s17_5_game_ext.rs`）

headless 直驱（`open_headless` + 真场景 `load_scene` + `attach_all_with_sources`
+ `mount_input_view`；逐帧 `collect_input` → `emit_input_signals` →
`step_headless` → `update_extensions`——与游戏宿主逐字同序；扩展面无 GPU
依赖，headless/窗口同一条路径）。真资产 skip-if-missing 惯例。

| 编号 | 用例 | 断言（全部实测过） |
|---|---|---|
| T-GE-01 | Dodge + 真盘装载 shake.js | 装载计数 == 1 且自报 id=shake；300 帧诊断清单恒空、`extension_faults() == 0`；敌人 AI headless 下真实逼近（e3 位移 > 50px，非测试摆拍；player 无输入不动）；**震动发生于帧 54**（首次 cam 偏离基准 > 0.5px，实测最大偏移 ≈3.9px）；震后相机逐位回基准（存在 dev == 0.0 帧） |
| T-GE-02 | Mini Dungeon + **同一份文件** | 同款断言全过（最大偏移 ≈3.6px、震后归位、零诊断零故障）——跨游戏通用性的正面证据 |
| T-GE-03 | 坏扩展不挡游戏（临时目录 Extensions/） | 语法错误扩展装载报错（`failed == 1`）且缺席；宿主扫描惯例下同目录好扩展照常装载（`loaded == 1`、计数 1）；30 帧游戏循环零诊断、好扩展每帧真实驱动树（player.x 恰 +1/帧）、faults == 0 |

Enemy-approach 查证结论（任务项 3 的前置问题）：**敌人 AI 在 headless 下
真实向玩家移动**（e1/e2/e3_brain 的 `on "tick"` 追踪脚本经 `step_headless`
内建 tick 驱动），故 T-GE-01 用自然逼近，无需 set_local 摆拍——set_local
直改树作为宿主测试手段留作备选未用。

## §4 门禁（全部实测，worktree 内）

| 门禁 | 结果 |
|---|---|
| 十 crate `cargo test --release` | **731 passed / 0 failed**：nes-asset 34、nes-audio 52、nes-extension-api 7、nes-extension-js 24、nes-media 27、nes-render-api 45、nes-render-extract 56、nes-render-wgpu 123、nes-scene 247、nes-runtime 116（基线 728 + 新增 3：T-GE-01/02/03） |
| clippy ×10 | 10/10 crate `--all-targets` **0 warning** |
| 守卫 `check_dependency_direction.py` | **15/15**（零新依赖） |
| first_game 冒烟 | `NES_GAME_FRAMES=300` 窗口跑满：控制台两行装载（hello、shake），零诊断行、零错误，干净退出 |
| dungeon_game 冒烟 | 同上（两行装载 + `[完成] Mini Dungeon 退出`） |
| editor_shell 冒烟回归 | ① `NES_GAME_FRAMES=120` 干净冒烟：两行装载 + 干净退出；② `NES_EDIT_DEMO=1 NES_EDIT_FRAMES=420` 全断言冒烟：挂载/卸载/enabled/折叠/刷新/play/stop/reset/音频/视频/IME/音乐全部过 |
| regression 基线 | `t_abi_01_dodge_baseline` + `t_gp_01_dodge_gameplay_and_determinism` 原样绿（基线 hash `b5b51dac27012ace` 不动） |

**冒烟适配说明（唯一一笔非新增改动）**：`Extensions/shake.js` 入库使编辑器
res:// 树多一行（字典序在 Scripts/ 之上），`NES_EDIT_DEMO` 的 FileSystem
双击固定坐标随行高平移（S13/S15 同款先例）：files 档 y 289→307、默认档
292→310，注释同步。断言面不变。

## §5 遗留（按裁决口径写明）

1. **每游戏阈值配置化**：shake.js 的 CONFIG（阈值/时长/幅度/节点名表）是
   源码常量——两游戏共用同一组数字。后续可做扩展清单（manifest：同目录
   `<name>.json` 声明 CONFIG 覆盖）或 `nes.registerExtension` 的宿主传入
   配置面（需新能力 trait，additive）——本期不做，改数字改源码即可。
2. **扩展市场目录**：`Extensions/` 目前是单一扁平目录（启动全装载）。市场
   形态（按游戏启用子集、版本、依赖声明）需要 manifest + 宿主启用面——归
   扩展生态后续期。
3. **更多通用扩展**：shake.js 证明了"读快照 + 写队列 + 协程"足以承载跨游戏
   玩法增强。同族候选：屏幕边缘受击闪白（`setVisible` 面）、慢动作（需时间
   缩放能力面——现有能力面没有，是真正的缺口）、回放记录器（需要扩展自有
   存储——S17.3 遗留 3 的 C6）。
4. **震动期间的游戏相机脚本**：扩展以触发时基准归位；若宿主游戏有相机跟随
   脚本，震动写会与脚本写每帧互踩（扩展写下一帧呈现、脚本写当帧生效——
   实际观感是脚本赢）。通用解法是"相机偏移层"能力面（渲染侧 offset，不写
   树）——归渲染后续期；两样例游戏无相机脚本，本期不受影响。
5. **`Math.random` 与确定性**：shake.js 用了 `Math.random`（P0 文档警告不
   禁用）。它不在任何指纹基线路径（headless CLI / `run_headless` 不装载扩
   展），但未来若要"带扩展的确定性回放"，需沙箱分区（S17 第 1 期遗留项）
   先落地。
