# NES 2.0 · S11-1 脚本级只读共享面 v1

> 交付日期：2026-10-02　｜　状态：**F-1 最小扩展落地——点读回退 + 写纪律冻结；T-SHARE-01..03 契约回归**
> 前置：S11-0 API 摩擦重评（F-1 确认 B 类）；S8.2b v1.1（无第二状态总线裁决）；M1 元不变量。

---

## 0. 一句话结论

S11-0 审计确认的 F-1（跨脚本局部不可读，唯一 B 类缺口）以**最小
扩展**落地：`game.gold` 点读语法在属性表未命中且目标是 Script 节点时
**回退读该脚本的局部变量**。写侧不动——属主局部只有自己的 `SetLocal`
能写，跨脚本写走既有属性纪律（无第二状态总线，S8.2b v1.1 裁决保持）。
四处调用点共享同一 `SharedStates` 表； Dodge ABI 基线指纹不变。

---

## 1. 语义冻结

### 1.1 点读优先级（新增回退链）

```text
game.gold（GetProp 语义，game 是 Script 节点）
  1. 属性表命中（schema 键 / set_prop_raw 落的键）→ 属性值
  2. 属性未命中 + 目标是 Script 节点 →
     a. 查自己（node == host）→ 手中 locals（执行期已被取出）
     b. 查别人 → shared 表里该脚本的 locals
  3. 缺名 → I64(0)（不停机——与既有 GetProp 缺省同口径）
  非 Script 节点：无回退（维持旧契约）
```

**属性优先于局部**是有意的：Inspector / 场景文件写的属性是引擎管辖
的公共面，必须权威；局部是脚本私有的便利暴露。同名时属性遮蔽点读
（T-SHARE-02 断言），但脚本内部裸标识符永远读自己的局部（编译层
`Local` 指令，不经 GetProp）——两条读路径互不干扰。

**自引用统一**：执行期间脚本自己的局部被取出（remove → run →
insert），shared 表里没有自己——回退链 a 分支直接看手中 locals。
`game.gold` 在 game 内部和外部读到同一个值，无歧义。

### 1.2 写纪律（不变，显式冻结）

| 写法 | 落点 | 结果 |
|---|---|---|
| `gold = v`（属主脚本内） | 自己的 SetLocal | 局部更新 ✓ |
| `game.gold = v`（跨脚本） | Cmd SetProp → schema 校验 | schema 键落地 ✓；未知键**静默丢弃**（写错不崩帧——既有纪律）|
| 任何路径写别人局部 | —— | **不存在此指令**（防第二状态总线）|

跨脚本的**可写**共享状态走属性平面（schema 键）或信号载荷——这两条
是既有通路，F-1 没有新增写面。可写共享的需求（如 dodge 的 hp 归
referee 管）用属主模式：状态住属主脚本，别人只读（`referee.hp`），
变更经信号通知。

### 1.3 时序

process（阶段 4）先跑、局部落表；信号泵（阶段 5）后读——读到的是
**当帧终值**（T-SHARE-01：init 100 + 每帧 +1，预发 query 后 tick 1
读到 101）。跨脚本读无陈旧窗口：局部在属主 run 结束时立即回插
shared 表。

---

## 2. 实现（最小侵入）

`nes-scene/src/script.rs`：

1. **`SharedStates` 类型别名** = `Rc<RefCell<HashMap<NodeId,
   BTreeMap<String, Value>>>>`（就是既有 `ScriptVm::states` 的类型
   ——零新容器，零新分配策略）
2. **`run()` 加 `shared: &SharedStates` 参数**——四处调用点
   （信号 init / 信号主 / process init / process 主）全部传
   `self.states.clone()` 的引用。信号处理器闭包本就捕获 states 的
   Rc 克隆（S6 装载结构），无新增捕获
3. **`Op::GetProp` 回退链**（见 §1.1）——`kind_tag(node) == Script`
   判定后查 locals；`node == host` 看手中，否则查 shared

改动量：实现 ~40 行（含注释），测试 3 例。无新 Op、无新语法产生式
（`game.gold` 既有文法，`NodeByName + GetProp` 既有编译产物）、
无 schema 变更、无序列化变更。

### 为什么不加全局名空间（autoload 形态）

S11-0 曾建议 "全局脚本状态（类似 Godot autoload）"。实现时收缩为
**节点名寻址的局部回退**：autoload 需要全局注册表（新状态悬挂点），
而节点名寻址复用既有 `find_by_name` 与既有文法。代价是目标脚本必须
有名字——实践中管理器脚本（game / referee / hud）本就是命名节点，
零摩擦。

---

## 3. 契约回归（T-SHARE-01..03）

`nes-scene/tests/s11_share.rs`，全部用**源码级** `source` 属性装载
（非手拼 Op——验证真实文法 `game.gold`）：

| 编号 | 断言 |
|---|---|
| T-SHARE-01 | 信号脚本读 `every` 脚本局部：预发 + tick 1 读 101（process 先、泵后）；再 3 帧 + 末帧读 105（持续跟随）|
| T-SHARE-02 | 写纪律三面：跨脚本写未知键丢弃（prop None）；跨脚本写 schema 键（`sp.visible`）落地；属主局部免疫（7+4=11 不被 99 穿透）；`set_prop_raw` 同名属性遮蔽点读（读 50）而属主裸局部继续计数（12）|
| T-SHARE-03 | 自引用 `game.gold` 看手中局部（5）；缺名 `game.nope` 回 I64(0) 不停机（无 `__halt`）；非 Script 节点 `sp.nada` 维持旧契约（0）|

---

## 4. 回归证据

```text
nes-asset    34 通过 / 0 失败
nes-scene   213 通过 / 0 失败（含 T-SHARE-01..03 新 3 例）
nes-render-api    44 / 0
nes-render-extract 42 / 0
nes-render-wgpu   89 / 0
nes-runtime  55 通过 / 0 失败（Dodge ABI 基线指纹不变）
clippy      六 crate 全 0 警告
守卫        check_dependency_direction.py 11/11 通过
```

Dodge 基线不变的原因：回退只在"属性未命中 + Script 节点"时激活，
且缺名仍回 I64(0)——既有脚本的行为逐帧等价。

---

## 5. F-1 解决后各项目形态（回扣 S11-0 审计）

| 项目 | 原 workaround | 现在可以 |
|---|---|---|
| Dodge | hp 住 referee（管理器持状态）| 玩家脚本持 hp，referee 只读 `player.hp` |
| Mini Dungeon | gun+referee 合并膨胀 | score 住 referee，gun 只读 + 信号上报 |
| Seed & Harvest | ~1800 字符单 tick 入口 | gold 住 game，种植/收获各自脚本读 |
| Editor Shell | 宿主 Rust 直读 | 状态脚本化（读侧不再绕过 VM）|

## 6. 后续

- **第三完整游戏**（S11-2 方向）：用 F-1 拆多脚本协作——验证扩展
  非过拟合（不是为 farm 量身定做）
- **F-4 编辑器工具**（A 类）：Inspector "从文件挂载脚本" —— 后置
- **F-8 for_each 性能**（D 类）：无实测瓶颈，不动

## 7. 里程碑记注（M4 口径）

- 渲染侧零改动（F-1 纯 nes-scene）
- 提取/消费管线零改动
- 唯一跨里程碑影响：`script.rs` 的 `run()` 签名（私有函数，无
  外部调用者——examples/editor_shell 均经 SceneTree/ScriptVm 公共面）
