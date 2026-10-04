# NES 2.0 · S17 扩展运行时第 1 期（JS 扩展：QuickJS-NG + rquickjs）v1

## §0 结论

**S17 第 1 期达成：NES 有了第一个真正的 JS 扩展运行时，且 Extension API 已冻结。**

- `hello.js`（仓库首个 JS 扩展）在 QuickJS-NG 里运行：注册扩展 → 每帧
  `nes.scene.find("obj1")` + `nes.node.setPos(...)` 驱动方块转圈 → 按住
  空格经 `nes.input.isPressed` / `nes.audio.play` 播 `Audio/beep`（0.5）。
- 冒烟：headless 300 帧 + 窗口 300 帧全过（树状态断言：方块位移
  111.49px；JS 异常帧 0）；**两次 headless 运行终态逐位一致，headless 与
  窗口模式终态逐位一致**（146.41873, 147.01727）—— "同一运行时、同一
  tick 语义"的口径延伸到了扩展面。
- 依赖分层政策落成三层（用户裁决）：Core 零第三方 / Official Adapter
  白名单（nes-media、nes-extension-js 两叶）/ 其余全部冻结面。守卫
  **13 → 15**（新增 G14/G15），15/15 通过。
- 十 crate `cargo test --release` 全绿（基线 670 + 新增 23 = **693**），
  clippy 0 ×10。

git：阶段 A = `c048be9`，阶段 B = 见提交尾注（本文件随阶段 B 提交）。

---

## §1 用户裁决全文（架构口径，逐条落实）与三层政策

1. **NES Core：纯 Rust 零第三方，不变。JS 扩展：QuickJS。**
   → Core 七 crate 一行未动；JS 面选 QuickJS-NG。
2. **具体选型：QuickJS-NG + rquickjs**（高层 Rust 绑定，目标 QuickJS-NG
   非 Bellard 原版）。→ `nes-extension-js` 钉 `rquickjs = "0.9.0"`
  （rquickjs-sys 0.9 随包 QuickJS-NG C 源码，cc 就地编译，MSVC 直过）。
3. **QuickJS 绝不是 NES 的"第二 Runtime"**——只是 **Extension Execution
   Runtime**：JS 扩展只能通过 `nes` 能力对象操作（`nes.registerExtension`
   / `nes.scene.find` / `nes.input.isPressed` / `nes.audio.play` /
   `nes.node.get...`），**永远不直接碰 SceneTree/Runtime/Renderer/WGPU**。
   → 能力注入即边界：`nes` 对象只暴露能力 traits 桥出的函数；QuickJS
   堆里没有任何引擎类型；Rust 侧没有任何 JS 句柄。
4. **crate 层级**：`nes-extension-api`（纯 ABI，零依赖）→
   `nes-extension-js`（rquickjs 绑定）→ QuickJS-NG；
   `nes-extension-native`（原生扩展位）**占位未实现**（S17 文档即占位
   声明：`ExtensionLifecycle` trait 是未来原生扩展的宿主句柄）。
5. **可换 backend**：`trait JsRuntime { create_context / load_module /
   call / collect }` —— 上层不关心底下是 QuickJS-NG、Boa 还是
   quickjs-rs（后两者 Beta 不做）。→ trait 在 nes-extension-api 冻结，
   nes-runtime 只依赖该 trait 面。
6. **真正要冻结的是 Extension API，不是某个 JS 引擎。** → §2 的全部
   类型/trait 即冻结面；换引擎只动 nes-extension-js。
7. **依赖分层政策三层**：
   - Core：零第三方（G3/G10/G11/G12/G14 钉住）；
   - Official Adapter：白名单适配层，现有**两叶** —— nes-media（媒体
     解码，G13：image 系 + symphonia 系）与 nes-extension-js（JS 扩展
     运行时，G15：rquickjs 家族闭包）；
   - 其余：零第三方纪律不变。

---

## §2 nes-extension-api：冻结面全表

crate 纪律：零第三方、零仓库内依赖、无 build.rs、空 `[workspace]` 钉
独立根（G14）；`#![forbid(unsafe_code)]`。

### 2.1 值边界

```rust
pub enum NesValue {                       // 引擎 <-> 脚本唯一值通货
    Null,                                 // JS null/undefined 归一于此
    Bool(bool),
    F64(f64),                             // JS 数值唯一形态（整数也落这里）
    Str(String),
    Array(Vec<NesValue>),
    Object(Vec<(String, NesValue)>),      // 键保序（转换序 = 存储序）
}
// 便捷：str/arr/obj 构造器；as_bool/as_f64/as_str/is_null 读面。
// 二进制（字节串）P0 不含，后续加变体 = 非破坏扩条。
```

### 2.2 JsRuntime（backend 冻结面）

```rust
pub struct JsContextId(pub u64);          // backend 本地句柄

pub trait JsRuntime {
    fn create_context(&mut self) -> Result<JsContextId, ExtError>;
    fn load_module(&mut self, ctx: JsContextId, source: &str) -> Result<(), ExtError>;
    fn call(&mut self, ctx: JsContextId, function: &str,
            args: &[NesValue]) -> Result<NesValue, ExtError>;
    fn collect(&mut self);
}
```

**与用户草图的签名差异（ergonomics 微调，如实写明）**：`ctx` 以
`JsContextId`（Copy）**按值**传入而非引用 —— 句柄结构由各 backend 自行
解释。P0 语义约定：`load_module` = 全局脚本求值（顶层 function/var 落
全局符号；P0 扩展不是 ES 模块，import/export 不做）；`call` = 按名调
全局函数，值经 NesValue 双向转换；`collect` = backend 自选深度的 GC。

### 2.3 能力 traits（P0 最小集；宿主实现、JS 侧绑定）

```rust
pub struct NodeRef(pub u64);              // 不透明节点引用（位形见 §4）

pub trait SceneCapability {               // nes.scene.find
    fn find(&self, name: &str) -> Option<NodeRef>;
}
pub trait NodeCapability {                // nes.node.getPos/setPos/setVisible/getName
    fn get_pos(&self, r: NodeRef) -> Option<(f32, f32)>;
    fn set_pos(&mut self, r: NodeRef, x: f32, y: f32);   // [微调1] &mut self
    fn set_visible(&mut self, r: NodeRef, v: bool);      // [微调1] &mut self
    fn get_name(&self, r: NodeRef) -> Option<String>;
}
pub trait InputCapability {               // nes.input.isPressed
    fn is_pressed(&self, name: &str) -> bool;
}
pub trait AudioCapability {               // nes.audio.play（&self，草图原样）
    fn play(&self, key: &str, volume: f32);
}
pub trait ExtensionLifecycle {            // 扩展生命周期（宿主 -> 扩展）
    fn register(&mut self, id: &str);
    fn update(&mut self);                 // 每帧钩子（simulate 之后）
}
```

**[微调1]** 草图写 `set_pos(&self, ...)`；落地改 `&mut self` —— 防实现方
被迫上内部可变性（触发型的 `play` 保持 `&self`，实现方用 RefCell 记账即
可）。**[微调2]** 能力方法的 `NodeRef` 参数按值传（Copy 句柄，草图原为
`&NodeRef`）。两处均为防实现端凑合，语义零变化。

### 2.4 ExtError（中文 Display）

`RuntimeInit / ContextCreate / UnknownContext / Load / CallFailed /
Convert / Capability` 七变体，Display 全中文（"扩展模块装载失败：…"等），
实现 `std::error::Error`。第三方错误类型一律不出现在 ABI。

---

## §3 nes-extension-js：rquickjs 选型与构建实录

### 3.1 选型与版本事实

- crates.io 上 **0.9 系恰为 0.9.0 一版**（其后是 0.10~0.14 系，撰写时
  最新为 0.14.0）。任务书指向"最新 0.9 系"→ 钉 `rquickjs = "0.9.0"`；
  升系是纯 nes-extension-js 内部事务（冻结面不变），Beta 不做。
- rquickjs-sys 0.9.0 **默认特性即 QuickJS-NG C 源码就地编译**（`cc` 链：
  find-msvc-tools → cc → cl.exe），**bindgen 是可选特性且默认关闭**
 （绑定为预生成）—— 任务书预案里"bindgen 失败"的风险**不存在于默认
  路径**。
- 构建实录（探针：仅依赖 rquickjs 0.9.0 的 hello-world）：
  `cargo build --release` 一次通过，**29.2s**（冷），MSVC 工具链零配置。
  家族闭包（G15 实测 6 包）：rquickjs / rquickjs-core / rquickjs-sys /
  cc / shlex / find-msvc-tools。

### 3.2 构建之后的"坑"（API 实录，全部已解）

构建零坑，真正的坑在 0.9 的 API 面（相对 0.6~0.8 的资料口径）：

1. `Func::wrap` 不存在 → **`Func::new`**（`Func<T,P>: IntoJs`）。
2. `IntoJs` 的方法是 `into_js(&ctx) -> Result<Value>`；且
   **`Option<T>: IntoJs` 把 `None` 映射成 `undefined`**（`Null<T>` 包装
   类型只存在于参数侧，无 IntoJs impl）—— 能力契约里"未找到"必须是
   **null**（值语义，不是"没有返回"）→ find/getName/getPos 走具名生命
   周期辅助函数显式 `Value::new_null`。
3. **裸元组没有 IntoJs**（只有 `List<(…)>`）→ `getPos` 手工组
   `Array`（返回 `[x, y]`）。
4. **切片不作 1 元组展开传参**（与新版不同）→ 动态参数走
   `Args::new_unsized(ctx)` + `push_arg` + `Function::call_arg`。
5. `Runtime::set_memory_limit(usize)`（非 Option）、`run_gc()`（非
   `gc()`）、`keys::<String>()`（单泛型）。
6. JS 异常文本：`Error::Exception` 本身不带文本，必须**在
   `Context::with` 栈内** `ctx.catch()` → `Exception::from_object` →
   `message()/Display`（出栈拿不到挂起异常）。`classify_err` 统一收口：
   异常 → `ExtError::CallFailed("JS exception: Error: oops")`。
7. 生命周期：带 `Value<'js>` 返回的绑定闭包写不出 HRTB 注解 → **模式**：
   闭包不做返回类型注解、调一个具名生命周期的泛型辅助函数
  （`fn node_get_pos_js<'js>(…, ctx: Ctx<'js>, …) -> Result<Value<'js>>`），
   推断自动统一。
8. 跨 FFI 纪律：**闭包体零 panic**（`try_borrow` 失败要么静默让路、要么
   抛 JS 异常 —— panic 穿 C 边界是 UB，不可赌）。

### 3.3 能力对象注入（冻结的 JS 面）

```
nes.scene.find(name)             -> number(NodeRef) | null
nes.node.getPos(ref)             -> [x, y] | null      （tick 后快照）
nes.node.setPos(ref, x, y)       -> undefined          （写队列，§4）
nes.node.setVisible(ref, v)      -> undefined          （写队列）
nes.node.getName(ref)            -> string | null
nes.input.isPressed(name)        -> bool               （本帧输入快照）
nes.audio.play(key, volume)      -> undefined          （混音器直通）
nes.registerExtension(id)        -> undefined          （扩展自报身份）
nes.onUpdate(fn)                 -> undefined          （P0 单回调槽）
```

实现：7 个 `__nes_*` 扁平全局函数（`Func::new` 闭包持
`Rc<RefCell<dyn Capability>>`）+ 一段 `NES_BOOTSTRAP_JS`（ASCII）组装
`nes` 对象、update 蹦床（`__nes_update`）与扩展身份槽
（`__nes_extension_id`）。**关键决策：所有 JS 值（onUpdate 回调等）留在
JS 堆**（全局槽），Rust 侧零 JS 句柄 —— GC 安全由构造保证，不需要
Persistent 根；`JsExtension::update` 经冻结面 `call("__nes_update")` 触发
蹦床（同时证明了冻结面 P0 足够）。

`RquickjsRuntime` 资源纪律：内存上限 64 MiB、栈上限 1 MiB（失控脚本的
损失上界 —— 沙箱分区之外的第一道闸）；每扩展一个 Context（互相隔离），
能力桥按扩展共享。

---

## §4 nes-runtime 接线（能力桥 + ExtensionManager）

### 4.1 读快照 / 写队列（安全借用的确定性解法）

QuickJS 闭包是 'static 的，无法安全持有 `&mut SceneTree`。桥的口径：

- **读**（find/getPos/getName/isPressed）：每帧从 **tick 后的树**快照取
  答（`ExtCapsState::refresh`：名字->NodeRef、位置/名字表、按住键投影、
  混音器句柄）—— 扩展看到的是当 tick 后状态，帧内自洽；
- **写**（setPos/setVisible）：**操作队列**，update 阶段结束的同一帧内
  按提交序 `apply`（`set_local` 只改平移分量，保留旋转/缩放/斜切；
  可见性走 `visible` 属性）—— **扩展写树 = 游戏状态，进指纹**；
- **音频**（play）：直通混音器（`Arc<Mutex>` 克隆，不借树）—— 音频不进
  语义状态（S13 裁决）；键未注册静默丢弃。

**NodeRef 位形（裁决要求写明）**：`u64 = NodeId::to_bits()` =
`(generation << 32) | slot`。选它而非裸 slot 的理由：generation 位防
"悬垂句柄撞上复用槽位的另一个节点"（单元测试
`dangling_node_ref_is_inert` 钉住：伪造代际的句柄读不到、写不落地）。
JS 侧 number 为 double，精确承载 < 2^53 —— P0 位形远小于此，风险由文档
声明（generation 到 2^21 之后才会越界）。

**帧序契约**：`NesRuntime::update_extensions` 在 **simulate 之后**调用
（窗口模式下即 `frame*` 返回后）—— 扩展看到当 tick 后状态；写队列当帧
落地、下一帧呈现（一帧延迟，如实写明）。

**确定性论断（如实写明前提）**：headless 下 JS 执行是确定性的 —— 同源
码同输入 → 同字节码 → 同求值序 → 同写队列 → 同树状态 → 同指纹。前提：
脚本不依赖宿主注入的不确定源；`Date` / `Math.random` 在 QuickJS 内建里
**P0 不禁用但文档警告**（沙箱分区/内建裁剪是后续期次）；时间也是输入
（固定 delta）。实测：两次 headless 运行终态逐位一致，且与窗口模式一致。

### 4.2 装配与生命周期

- `NesRuntime::load_extension_file(path)`：读盘 → 惰性构造
  `ExtensionManager`（不开扩展零开销，与音频面同一纪律）→ 每扩展独立
  Context + 能力注入 + 顶层求值 + `JsExtension::register`（JS 自报 id
  优先，缺省用文件名）。
- `NesRuntime::update_extensions() -> Vec<String>`：快照 → update 钩子 →
  写队列落地；返回 JS 异常清单（不炸帧，宿主处置；冒烟断言为空）。
- `NesRuntime::extension_count()`。
- G11 白名单同步扩两条（runtime ──▶ nes-extension-api /
  nes-extension-js，path），守卫文案改为"白名单内项目 crate"。

### 4.3 hello.js 全文（入库：examples/assets/Extensions/hello.js）

```javascript
// NES 2.0 S17 demo extension (ASCII only, per repo discipline).
nes.registerExtension("hello");

var angle = 0.0;
var radius = 60.0;
var omega = 0.05;
var center = null;

nes.onUpdate(function () {
  var ref = nes.scene.find("obj1");
  if (ref === null) {
    return;
  }
  if (center === null) {
    center = nes.node.getPos(ref);
  }
  angle = angle + omega;
  var x = center[0] + Math.cos(angle) * radius;
  var y = center[1] + Math.sin(angle) * radius;
  nes.node.setPos(ref, x, y);

  if (nes.input.isPressed("Space")) {
    nes.audio.play("Audio/beep", 0.5);
  }
});
```

（混音器键约定 = 资源路径去扩展名：`Audio/beep.wav` → `Audio/beep`，
任务书示意里的 `"beep"` 即此键的短写。）

### 4.4 ext_demo（examples/ext_demo.rs）

代码搭树（相机 + obj1 精灵 @ 192,108）→ 装载 hello.js → 帧循环
（collect_input → emit_input_signals → simulate → **update_extensions**）；
帧 100 按下/帧 130 抬起空格（输入 + 音频路径走真）。双模式：
`--headless`（open_headless + step_headless，缺省 300 帧，无 GPU 依赖）
与窗口模式（NES_GAME_FRAMES=300 冒烟）。断言：方块位移 > 5px + JS 异常
帧 = 0。

---

## §5 门禁（全部实测）

| 门禁 | 结果 |
| --- | --- |
| 守卫 check_dependency_direction.py | **15/15**（G1–G13 保持，G14/G15 新增；G13 文案修订为"两个白名单适配层之一"；G11 白名单九 crate） |
| 十 crate `cargo test --release` | **全绿，合计 688 通过 / 0 失败**（基线 670 + 新增 18：nes-extension-api 6、nes-extension-js 7、nes-runtime extension 模块 5） |
| clippy（10 crate × all-targets） | **0 警告 0 错误 ×10** |
| ext_demo 冒烟（headless 300 帧） | 通过：位移 111.49px、JS 异常帧 0、**双跑终态逐位一致** |
| ext_demo 冒烟（窗口 300 帧） | 通过：终态与 headless **逐位一致** |
| editor_shell 冒烟（回归，120 帧） | 通过（`[完成] Editor Shell 退出`） |

（分 crate 计数：nes-asset 34 / nes-scene 244 / nes-render-api 45 /
nes-render-extract 56 / nes-render-wgpu 123 / nes-audio 52 / nes-media 27 /
nes-runtime 94 / nes-extension-api 6 / nes-extension-js 7；含 doc-test 的
0 计行，总数以各 `test result` 行求和为准。）

新增测试的价值口径：nes-extension-api / nes-extension-js 的 13 个测试
**全部不需要引擎在场**（mock traits 驱动）—— 这正是"API 冻结"的验收
方式；nes-runtime 的 10 个测试用真 SceneTree 钉快照/队列/悬垂句柄/无引擎
端到端循环。

---

## §6 遗留（按裁决口径写明）

1. **沙箱分区 / 权限模型**（B 之后的下一站）：P0 只有内存/栈上限 +
   能力面最小化；`Date`/`Math.random` 未禁用（文档警告）；无每扩展
   权限声明、无 CPU 指令预算、无模块隔离装载（每扩展一个 Context 已是
   雏形）。
2. **C4-C5 让出与 hat**（生态扫描清单项）：扩展的协作式让出（yield）
   与 hat 形事件订阅未做 —— P0 只有单 update 槽。
3. **扇区化打包**：扩展作为资产格式（.zip/.nesext、清单、签名）未做；
   P0 = 裸 .js 文件 + 文件名兜底 id。
4. **AMV 之外的生态项**：扩展注册表/市场面、跨扩展通信（事件总线桥）、
   热重载（扩展文件变更 → 重载 Context）、多 update 槽/优先级、
   `nes-extension-native` 原生扩展位（trait 已留位）。
5. **rquickjs 升系**：0.14.0 已在（0.9 系冻结在 0.9.0）；升系是纯
   nes-extension-js 内部事务，Beta 不做。
6. **getPos 的一帧延迟**：扩展写在渲染后落地、下一帧呈现 —— 若未来
   需要当帧可见，把 update_extensions 挪到 simulate 与 extract 之间即可
  （帧序契约已在代码文档写明两个位置的语义差）。
