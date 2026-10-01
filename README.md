# NES 2.0

零第三方依赖的 2D 引擎内核（Rust），以**契约测试**为第一公民。
渲染走 wgpu-native 的手写 C-ABI FFI（`#[repr(C)]` + `LoadLibraryW` 运行时
符号解析，DLL 厂商包在 `wgpu-win/`），场景层是数据导向的枚举分发节点树。

## Crate 地图（单向依赖链，守卫 G1~G11 钉死）

```text
nes-asset ──▶ nes-scene ──▶ nes-render-extract ──▶ nes-render-api ──▶ nes-render-wgpu
     └────────────┴──────────────┴────────────────────┴──────────────┘
                        全部被 nes-runtime 消费（顶端叶子）
```

| Crate | 职责 |
|---|---|
| `nes-asset` | 资产身份（AssetKey：槽位+代际+类别）、磁盘加载（FsLoader）、内容戳热重载 |
| `nes-scene` | 场景树（延迟结构变更 + 命令缓冲）、schema 属性、tick 生命周期（enter/ready/process、暂停/时间缩放）、RON 序列化（含子场景嵌套与实例级覆盖） |
| `nes-render-extract` | 语义 -> 渲染物的唯一翻译点（每帧提取，方案 D） |
| `nes-render-api` | 渲染命令契约（RenderServer trait、DrawKey 全序、I1~I10 不变量） |
| `nes-render-wgpu` | wgpu-native 后端：图集/注册表/精灵管线/文本/控件 + Win32 窗口与 Surface（手写 FFI） |
| `nes-runtime` | 组装层：场景树 + 磁盘资产驱动整条帧循环（离屏与窗口共用一条绘制路径） |

## 验证

```bash
# 依赖方向守卫（11 条规则）
python check_dependency_direction.py

# 六 crate 测试（GPU 用例在无 DLL 环境自动跳过）
cd nes-asset  && cargo test   # 34
cd nes-scene  && cargo test   # 180
cd nes-render-extract && cargo test  # 42
cd nes-render-api    && cargo test  # 44
cd nes-render-wgpu   && cargo test  # 88
cd nes-runtime && cargo test        # 49

cargo clippy --all-targets   # 各 crate 零警告
```

## 演示

```bash
cd nes-runtime
cargo run --example engine_window      # 真窗口帧循环 + 动画（SceneObserver）+ 热重载
cargo run --example script_panel       # 编辑器脚本面板（窗口键入 -> 提交 -> 热重载 -> 同帧像素）
cargo run --example first_game        # 首个真实游戏 Dodge（方向键走位，场景+行为全在 first_game.ron）
cargo run --example dungeon_game      # 移植压力测试 Mini Dungeon（走位+射击+拾取+门）
cargo build -p nes-runtime --release --bin nes   # headless CLI
./nes-runtime/target/release/nes.exe --headless scene.ron --frames 300   # 逐帧状态哈希（确定性）
cargo run --example scene_disk         # 磁盘场景 -> 渲染 -> 回存
cargo run --example subscene_reload    # 子场景热重载（改文件 -> 整树重载）
```

## 文档

里程碑交付文档在本目录（`NES2.0_*.md`）：接口草案、M4 渲染接入各阶段、
M5 引擎组装层、S6 系列（窗口/Surface、tick 接线、序列化闭环、暂停与时间
缩放、子场景嵌套/热重载/实例级覆盖/diff 回写/结构性覆盖）。每份文档含
出口准则、语义裁决记录与遗留清单。

*（内容由AI生成，仅供参考）*
