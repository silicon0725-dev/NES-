//! NES 2.0 引擎组装层：把五个 crate 装配成一条帧循环。
//!
//! # 这一层是什么
//!
//! M1~M4 交付了五个各司其职的 crate，但至今"渲染"都由测试与示例**手工驱动**
//! 服务端。本 crate 补上最后一块拼图 —— **真实场景树驱动整条管线**：
//!
//! ```text
//! SceneTree（节点 + 属性 + 资源声明）
//!   └─ bind: ResourceTable -> AssetRegistry（FsLoader 从磁盘加载字节）
//!        └─ upload: BMP 解码 -> TextureRegistry（按键注册 GPU 纹理，键位 =
//!           AssetKey::as_render_key 的位镜像，与提取层 `render_key_of_bits` 同源）
//!   └─ frame: SceneTree::tick（结构落地 + enter/ready/process 生命周期 +
//!        变换冲洗，宿主行为经 SceneObserver 挂入）-> RenderExtractor::extract_into
//!        （语义 -> 渲染物的唯一翻译点，推给 WgpuRenderServer 并产出命令流）
//!        -> CommandConsumer::consume（命令 -> 清屏/精灵/控件/文本 -> 离屏读回）
//! ```
//!
//! # 不做什么（边界纪律）
//!
//! - **不实现语义**：变换、z 序、相机、锚点、排版的算式分别冻结在
//!   `nes-scene` / `nes-render-api`；本层只搬运与组合；
//! - **不改上游**：五个 crate 一行不动（本层是它们之上的新叶子，G11 钉住）；
//! - **纹理解码口径**：资产以 **BMP** 交付（`nes-render-wgpu::bmp` 手写解析，
//!   零依赖纪律）；其他格式由外部预处理转换（见各示例的资产生成脚本说明）。
//!
//! # 帧循环
//!
//! [`NesRuntime::frame`] 一帧做四件事：tick（结构落地 + 生命周期 + 变换冲洗，
//! [`NesRuntime::frame_with`] 可挂宿主行为观察者）-> 提取（含 `submit_into`）
//! -> GPU 消费。命令缓冲与提取缓冲跨帧复用（稳态零分配，方案 D 第三件套）。
//!
//! # 热重载
//!
//! [`NesRuntime::poll_reloads`] 轮询文件变化（内容戳判定），配合
//! [`NesRuntime::upload_pending_textures`] 把新版本字节重传 GPU —— 引擎侧
//! "改磁盘文件 -> 下一帧画面变化"的完整链路。

use std::collections::BTreeMap;
pub mod headless;

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use nes_asset::{AssetKey, AssetKind, AssetRegistry, FsLoader, ReloadReport};
use nes_render_api::input::{InputCollector, InputSnapshot};
use nes_render_api::{FrameInfo, RenderAssetKey, RenderCommand, Vec2};
use nes_render_wgpu::FrameStats;
use nes_render_extract::RenderExtractor;
use nes_render_wgpu::bmp;
use nes_render_wgpu::{
    BackendError, CommandConsumer, FrameOutcome, GpuContext, RenderTarget, SpriteAtlas,
    SurfaceTarget, WgpuRenderServer,
};
use nes_render_wgpu::window::{drain_input, Window};
use nes_scene::scene_io::{instantiate_doc_with_resources, parse_ron, write_ron_with_resources};
use nes_scene::{
    AdoptReport, BindReport, NoObserver, PackOptions, ResId, ResourceTable, SceneDoc, SceneObserver,
    SceneTree, ScriptVm, TableError, Value,
};

pub use headless::{run, HeadlessReport};

/// 组装好的引擎帧循环。
///
/// 装配一次、逐帧推进；`tree_mut` / `resources_mut` / `registry_mut` 暴露给
/// 宿主做场景编辑与资产管理，`frame` 消费当前状态产出一帧像素。
pub struct NesRuntime {
    tree: SceneTree,
    table: ResourceTable,
    registry: AssetRegistry,
    extractor: RenderExtractor,
    server: WgpuRenderServer,
    /// 渲染消费端（GPU）。**headless 装配没有它**（[`Self::open_headless`]）——
    /// 渲染路径如实报错，语义路径（tick/输入/装载/指纹）完全同一套。
    consumer: Option<CommandConsumer>,
    commands: Vec<RenderCommand>,
    /// 资产根（场景文件与资源路径都相对它解析；`load_scene`/`save_scene` 用）。
    root: std::path::PathBuf,
    /// 已声明的纹理槽位（上传遍历用，声明序确定）。
    texture_slots: Vec<ResId>,
    /// 槽位 -> 已上传的资产版本（热重载判定：版本变化才重传）。
    uploaded_version: BTreeMap<ResId, u32>,
    /// 窗口模式（`open_windowed` 装配；析构序在消费器之后）。
    window: Option<Window>,
    /// 窗口呈现目标（窗口模式）。
    surface: Option<SurfaceTarget>,
    /// 当前场景的磁盘来源（`load_scene` 记录、`instantiate_scene` 清除）——
    /// 子场景热重载的入口：任一 Scene 类资产变化 -> 从来源整树重载。
    scene_source: Option<String>,
    /// 输入折叠器（S7.2：平台事件流 -> 帧快照）。
    input_collector: InputCollector,
    /// 输入快照共享槽（键探针读它 —— `mount_key_probe` 接线）。
    input_state: Rc<RefCell<InputSnapshot>>,
    /// 固定模拟步长（S8.1；`None` = 宿主纪律模式：帧 delta 原样一步）。
    fixed_step: Option<f32>,
    /// 蓄步器余量（fixed_step 模式下跨帧携带）。
    step_remainder: f32,
    /// 螺旋钳制丢弃的模拟步累计数（S8.1 如实计数）。
    steps_dropped: u64,
}

impl NesRuntime {
    /// 装配（离屏目标 `width` x `height`，资产根 = 当前目录）。
    pub fn open(width: u32, height: u32) -> Result<Self, BackendError> {
        Self::open_with_root(Path::new("."), width, height)
    }

    /// 装配（资产根目录指定；场景文件的相对资源路径都在该根内解析）。
    pub fn open_with_root(
        root: &Path,
        width: u32,
        height: u32,
    ) -> Result<Self, BackendError> {
        Self::assemble(root, width, height)
    }

    /// 窗口模式装配（资产根 = 当前目录）。
    pub fn open_windowed(
        title: &str,
        width: u32,
        height: u32,
    ) -> Result<Self, BackendError> {
        Self::open_windowed_with_root(Path::new("."), title, width, height)
    }

    /// 窗口模式装配（资产根目录指定），帧循环走 [`Self::frame_windowed`]。
    ///
    /// 离屏目标仍按客户区尺寸装配（`frame()` 可照常用于断言/截图），
    /// 窗口路径与离屏路径共用同一条提取/命令/渲染语义。
    /// 装配序：GPU/离屏目标先就位（失败时不闪窗口），再开窗口、再建表面。
    pub fn open_windowed_with_root(
        root: &Path,
        title: &str,
        width: u32,
        height: u32,
    ) -> Result<Self, BackendError> {
        let mut rt = Self::assemble(root, width, height)?;
        let window = Window::open(title, width, height)?;
        let Some(consumer) = &rt.consumer else {
            return Err(BackendError::ConfigMismatch(
                "窗口模式需要 GPU 装配（此处不可达：assemble(true)）".into(),
            ));
        };
        let surface = SurfaceTarget::new(consumer_ctx(consumer), &window)?;
        rt.window = Some(window);
        rt.surface = Some(surface);
        Ok(rt)
    }

    /// 公共装配：场景树/资产表/提取器/服务端/消费器（窗口与表面留空）。
    fn assemble(
        root: &Path,
        width: u32,
        height: u32,
    ) -> Result<Self, BackendError> {
        Self::assemble_with(root, width, height, true)
    }

    /// headless 装配：**同一运行时、同一 tick 语义，无 GPU/窗口**。
    ///
    /// 架构口径（S7.3）：headless 不是"窗口关掉继续跑"，也不是第二套
    /// 运行时 —— 只是不装配渲染端。`frame*`/`upload_pending_textures`
    /// 如实报错；装载/输入/tick/指纹与窗口模式逐字节同路径。
    pub fn open_headless(root: &Path) -> Result<Self, BackendError> {
        Self::assemble_with(root, 0, 0, false)
    }

    fn assemble_with(
        root: &Path,
        width: u32,
        height: u32,
        gpu: bool,
    ) -> Result<Self, BackendError> {
        let consumer = if gpu {
            let ctx = GpuContext::open()?;
            let target = RenderTarget::with_size(&ctx, width, height)?;
            let atlas = SpriteAtlas::new(&ctx)?;
            Some(CommandConsumer::new(ctx, target, atlas)?)
        } else {
            None
        };
        Ok(Self {
            tree: SceneTree::new("root"),
            table: ResourceTable::new(),
            registry: AssetRegistry::new(FsLoader::new(root)),
            extractor: RenderExtractor::new(),
            server: WgpuRenderServer::new(),
            consumer,
            commands: Vec::new(),
            root: root.to_path_buf(),
            texture_slots: Vec::new(),
            uploaded_version: BTreeMap::new(),
            window: None,
            surface: None,
            scene_source: None,
            input_collector: InputCollector::new(),
            input_state: Rc::new(RefCell::new(InputSnapshot::default())),
            fixed_step: None,
            step_remainder: 0.0,
            steps_dropped: 0,
        })
    }

    /// 窗口模式推进一帧（无行为代码）：泵消息 -> tick -> 提取 -> 渲染到表面 -> 呈现。
    ///
    /// 返回 `Ok(None)` 表示窗口已关闭（帧循环应停止）；`Ok(Some(stats))`
    /// 为本帧统计。**注意**：场景相机的视口应与窗口客户区一致（由宿主在
    /// 搭场景时设置），本方法不代改相机。
    pub fn frame_windowed(
        &mut self,
        frame: &FrameInfo,
    ) -> Result<Option<FrameStats>, BackendError> {
        self.frame_windowed_with(frame, &mut NoObserver)
    }

    /// 窗口模式推进一帧（宿主行为经 [`SceneObserver`] 挂入），阶段序同
    /// [`Self::frame_with`]，仅消费端换成窗口表面。
    pub fn frame_windowed_with(
        &mut self,
        frame: &FrameInfo,
        obs: &mut dyn SceneObserver,
    ) -> Result<Option<FrameStats>, BackendError> {
        let Some(window) = &self.window else {
            return Err(BackendError::ConfigMismatch(
                "非窗口模式运行时（用 open_windowed 装配）".to_string(),
            ));
        };
        if !window.pump() {
            return Ok(None);
        }
        if self.surface.is_none() {
            return Err(BackendError::ConfigMismatch("窗口模式缺少表面".to_string()));
        }
        let _steps = self.simulate(frame.delta, obs);
        self.extractor.extract_into(
            &mut self.tree,
            &self.table,
            &mut self.server,
            frame,
            &mut self.commands,
        );
        let Some(consumer) = &mut self.consumer else {
            return Err(BackendError::ConfigMismatch(
                "headless 运行时没有渲染端（用 open_windowed 装配窗口模式）".into(),
            ));
        };
        // 表面借用放在 simulate/extract 之后（整 self 可变借用的墙后面）。
        let surface = self.surface.as_ref().expect("上面已判存在");
        let stats = consumer.consume_to_surface(&self.commands, surface)?;
        Ok(Some(stats))
    }

    /// 场景树（宿主在此搭节点、写属性、改变换）。
    pub fn tree_mut(&mut self) -> &mut SceneTree {
        &mut self.tree
    }

    /// 资源声明表。
    pub fn resources_mut(&mut self) -> &mut ResourceTable {
        &mut self.table
    }

    /// 资产注册表（加载状态、事件订阅、重试等）。
    pub fn registry_mut(&mut self) -> &mut AssetRegistry {
        &mut self.registry
    }

    /// GPU 消费器（设置默认字体、注册表诊断等宿主侧操作）。
    /// headless 装配如实报 `None`（不装渲染端就没有消费器）。
    pub fn consumer_mut(&mut self) -> Option<&mut CommandConsumer> {
        self.consumer.as_mut()
    }

    /// 声明一张纹理资源（记录槽位，供 [`Self::upload_pending_textures`] 遍历）。
    pub fn declare_texture(&mut self, path: &str) -> Result<ResId, TableError> {
        let id = self.table.declare(path, AssetKind::Texture)?;
        self.texture_slots.push(id);
        Ok(id)
    }

    /// 从磁盘加载场景文件（相对资产根的 RON），实例化并**替换**当前树与资源表。
    ///
    /// `sub_scene` 引用（子场景嵌套，草案 §10）在实例化前**递归展开**：被引
    /// 场景文件相对资产根读取、槽位重编号合并（父子文件槽位号各管各的）。
    /// 展开失败（文件缺失/解析失败/循环引用）如实报错并指名路径。
    /// 返回资源体检报告（悬垂引用 / 类别冲突如实暴露，是否接受由宿主裁决，
    /// [`AdoptReport::is_clean`] 可作门槛）。纹理槽位按文档声明序重建上传
    /// 队列，上传账目清零；之后宿主照常 `bind_assets` + `upload_pending_textures`。
    /// 替换是全量的：当前树/表被丢弃 —— 场景文件是事实来源。
    pub fn load_scene(&mut self, rel_path: &str) -> Result<AdoptReport, BackendError> {
        let full = self.root.join(rel_path);
        let text = std::fs::read_to_string(&full)
            .map_err(|e| BackendError::Io(format!("读场景 {} 失败：{e}", full.display())))?;
        let doc = parse_ron(&text)
            .map_err(|e| BackendError::Io(format!("场景 {} 解析失败：{e}", full.display())))?;
        // 子场景展开：文件读取经闭包注入（相对资产根）。
        let root = self.root.clone();
        let expanded = nes_scene::expand_subscenes(&doc, &mut |rel| {
            let child = std::fs::read_to_string(root.join(rel)).map_err(|e| e.to_string())?;
            parse_ron(&child).map_err(|e| e.to_string())
        })
        .map_err(|e| BackendError::Io(format!("场景 {rel_path} 子场景展开失败：{e}")))?;
        let report = self.instantiate_scene(expanded)?;
        self.scene_source = Some(rel_path.to_string());
        Ok(report)
    }

    /// 场景文档 -> 当前运行时（树/表/纹理槽位/上传账目全量替换）。
    ///
    /// 文档里的 `Resource(n)` 槽位号原样复原 —— 属性引用、资源表、GPU 注册
    /// 表三处的身份位在磁盘往返后保持同源（M3 埋的线，这里接通到文件）。
    /// 文档直入没有磁盘来源：`scene_source` 清空（子场景热重载随之停摆，
    /// 直到下次 `load_scene`）。
    pub fn instantiate_scene(&mut self, doc: SceneDoc) -> Result<AdoptReport, BackendError> {
        let (tree, table, report) = instantiate_doc_with_resources(&doc)
            .map_err(|e| BackendError::Io(format!("场景实例化失败：{e}")))?;
        self.tree = tree;
        self.texture_slots = table
            .iter()
            .filter(|e| e.kind() == Some(AssetKind::Texture))
            .map(|e| e.id())
            .collect();
        self.table = table;
        self.uploaded_version.clear();
        self.scene_source = None;
        Ok(report)
    }

    /// 热重载轮询（场景侧，S6.7）：轮询资产注册表，任一 **Scene 类**资产
    /// （子场景文件，展开时已作为资源声明并绑定）内容变化时，从
    /// [`Self::load_scene`] 记录的来源**整树重载**：重新解析根文件、重新展开
    /// 子场景（读到新内容）、全量替换并重新绑定 —— 纹理重传账目随替换清零。
    ///
    /// 返回 `Ok(Some(来源路径))` 表示树已重建，宿主应接着
    /// [`Self::upload_pending_textures`]；`Ok(None)` 表示无场景变化。
    ///
    /// # 语义口径
    /// - **与 [`Self::poll_reloads`] 二选一**：本方法内部就做了一次全量轮询
    ///   （纹理变化也被消耗掉 —— 版本照常递增，随后的上传照常工作）；
    /// - 整树重载 = 场景文件是事实来源：**加载之后的宿主侧树编辑会被丢弃**；
    /// - 生命周期重放：下一 tick 全部节点重新 enter/ready；
    /// - 旧 `NodeId` 全部失效（新树新 arena），观察者持有的句柄需重新寻址；
    /// - **根场景文件自身的变更不在检测范围**（它不是自己 resources 段里的
    ///   资产条目）—— 宿主改根文件后重调 `load_scene` 即可；
    /// - `instantiate_scene` 直入的树没有来源：有变化也不重载（返回 `None`）。
    pub fn poll_scene_reload(&mut self) -> Result<Option<String>, BackendError> {
        let report = self.registry.poll_reloads();
        let scene_changed = report.reloaded.iter().any(|(key, _)| {
            self.table
                .iter()
                .any(|e| e.key() == Some(*key) && e.kind() == Some(AssetKind::Scene))
        });
        if !scene_changed {
            return Ok(None);
        }
        let Some(source) = self.scene_source.clone() else {
            return Ok(None); // 无磁盘来源：如实不重载（见语义口径）
        };
        self.load_scene(&source)?;
        let _ = self.bind_assets(); // 重载 = load_scene + bind（加载习语）
        Ok(Some(source))
    }

    /// 把当前树 + 资源表打包写成场景文件（相对资产根的 RON）。
    ///
    /// 与 [`Self::load_scene`] 互逆：树中属性引用的已声明槽位写进
    /// `resources` 段，悬垂槽位不写（它们是编辑器要修的缺口，不是文件事实）。
    /// **注意**：序列化的是**已落地**的结构 —— `add_node` 是延迟队列操作，
    /// 程序化搭树后须先 `tree_mut().apply_pending()`（或推进一帧）再保存，
    /// 否则新节点还挂在 pending 里，不会出现在文件中。
    pub fn save_scene(&self, rel_path: &str) -> Result<(), BackendError> {
        let text = write_ron_with_resources(&self.tree, &self.table, &PackOptions::verbose());
        let full = self.root.join(rel_path);
        std::fs::write(&full, text)
            .map_err(|e| BackendError::Io(format!("写场景 {} 失败：{e}", full.display())))
    }

    /// diff 式回写（S6.9）：把运行时对实例内部节点的编辑**烘焙**成包装节点的
    /// 覆盖记录。对每个绑定了 `sub_scene` 的包装节点：读当前磁盘上的子场景
    /// 文件（含其内部子场景的展开）-> 独立实例化作**参照** -> 与当前树 diff
    /// -> 记录全量重生成。返回更新的包装节点数。
    ///
    /// 之后 [`Self::save_scene`] 写出的覆盖即包含运行时编辑（存->载 复现
    /// 当前树）。保存本身保持纯读（S6.3 先例）—— 烘焙是显式动作：宿主也
    /// 可以**不**同步就保存 = 显式丢弃运行时对实例内部的编辑。
    ///
    /// 口径：结构差异（实例内增删节点）不产生记录（覆盖语义只管字段）；
    /// 参照取当前磁盘内容，见 `nes_scene::diff_instance_overrides` 文档。
    pub fn sync_overrides(&mut self) -> Result<usize, BackendError> {
        let wrappers = self.tree.preorder();
        let mut synced = 0usize;
        for wrapper in wrappers {
            // 绑定的 sub_scene 槽位（未绑定/无属性 -> 跳过）。
            let slot = match self
                .tree
                .props(wrapper)
                .and_then(|p| p.get(nes_scene::PROP_SUB_SCENE))
            {
                Some(nes_scene::Value::Resource(n)) if *n != 0 => *n,
                _ => continue,
            };
            let entry = self
                .table
                .iter()
                .find(|e| e.id().get() as u64 == slot)
                .cloned();
            let Some(entry) = entry else {
                continue; // 悬垂：加载侧已如实报告，烘焙跳过
            };
            if entry.kind() != Some(AssetKind::Scene) || entry.path().is_none() {
                continue;
            }
            let rel = entry.path().unwrap().as_str().to_string();
            // 参照 = 当前磁盘子场景（读 -> 展开 -> 独立实例化）。
            let text = std::fs::read_to_string(self.root.join(&rel))
                .map_err(|e| BackendError::Io(format!("读子场景 {rel} 失败：{e}")))?;
            let doc = parse_ron(&text)
                .map_err(|e| BackendError::Io(format!("解析子场景 {rel} 失败：{e}")))?;
            let root_dir = self.root.clone();
            let expanded = nes_scene::expand_subscenes(&doc, &mut |child| {
                let text = std::fs::read_to_string(root_dir.join(child))
                    .map_err(|e| e.to_string())?;
                parse_ron(&text).map_err(|e| e.to_string())
            })
            .map_err(|e| BackendError::Io(format!("展开子场景 {rel} 失败：{e}")))?;
            let (ref_tree, ref_table, _report) = instantiate_doc_with_resources(&expanded)
                .map_err(|e| BackendError::Io(format!("实例化参照 {rel} 失败：{e}")))?;
            let records = nes_scene::diff_instance_overrides(
                &self.tree,
                wrapper,
                &self.table,
                &ref_tree,
                &ref_table,
            );
            self.tree.set_instance_overrides(wrapper, records);
            synced += 1;
        }
        Ok(synced)
    }

    /// 把资源表接到资产注册表（注册 -> 加载 -> 场景持有；幂等）。
    pub fn bind_assets(&mut self) -> BindReport {
        self.table.bind(&mut self.registry)
    }

    // ---------- 游戏节拍（S8.1：内建 tick + 固定步长蓄步）----------

    /// 固定模拟步长上限保护（一帧至多补 5 步 —— 螺旋死亡钳制）。
    const MAX_STEPS_PER_FRAME: usize = 5;

    /// 设置固定模拟步长（S8.1）。设置后帧路径按蓄步器分步：
    /// `every`（process）的 delta **恒等于 step**（确定性模拟口径），
    /// 与渲染帧率解耦；未设置（缺省）保持宿主纪律模式（帧 delta 原样
    /// 一步 —— 既有宿主都传固定 1/60，行为不变）。
    pub fn set_fixed_step(&mut self, step: f32) {
        assert!(step.is_finite() && step > 0.0, "固定步长须为正有限值");
        self.fixed_step = Some(step);
        self.step_remainder = 0.0;
    }

    /// 螺旋钳制累计丢弃的模拟步数（如实观测）。
    pub fn steps_dropped(&self) -> u64 {
        self.steps_dropped
    }

    /// 推进一帧的模拟（**帧路径与 headless 共用的唯一节拍实现**）：
    ///
    /// 1. 发射**内建 `tick` 信号**（每帧恰一次，载荷 = 树帧号 ——
    ///    宿主级确定性事件；宿主不得再手发 `tick`，否则双交付）；
    /// 2. 按 `fixed_step` 蓄步分步调用 `SceneTree::tick`（未设置则
    ///    帧原样一步）：快帧可为 0 步（余量跨帧携带），慢帧补步
    ///    （上限 5，超限丢弃并计数）。
    ///
    /// 返回本帧执行的模拟步数。`tick_headless` 是不含内建 tick 的
    /// 裸单步（高级用途）；常规宿主走这里。
    fn simulate(&mut self, frame_delta: f32, obs: &mut dyn SceneObserver) -> usize {
        let (steps, delta) = match self.fixed_step {
            None => (1, frame_delta),
            Some(step) => {
                self.step_remainder += frame_delta.max(0.0);
                let n = (self.step_remainder / step).floor();
                self.step_remainder -= n * step;
                let n = n as usize;
                if n > Self::MAX_STEPS_PER_FRAME {
                    self.steps_dropped += (n - Self::MAX_STEPS_PER_FRAME) as u64;
                    (Self::MAX_STEPS_PER_FRAME, step)
                } else {
                    (n, step)
                }
            }
        };
        let frame_no = self.tree.frame() as i64;
        self.tree.emit_signal("tick", Value::I64(frame_no));
        for _ in 0..steps {
            self.tree.tick(delta, obs);
        }
        steps
    }

    /// headless 宿主的帧步进（含内建 tick 与蓄步；与窗口帧路径同一
    /// `simulate` 实现 —— 节拍语义无宿主分支）。返回本帧模拟步数。
    pub fn step_headless(&mut self, frame_delta: f32, obs: &mut dyn SceneObserver) -> usize {
        self.simulate(frame_delta, obs)
    }

    // ---------- 输入（S7.2：平台事件 -> 帧快照 -> 标准信号/探针）----------

    /// 收集本帧输入：排空平台事件队列 -> 折叠成快照（边缘 + 按住态）。
    ///
    /// 每帧调用一次（`frame*` 之前）；同时更新共享快照（键探针读它，
    /// 见 [`Self::mount_key_probe`]）。离屏模式同样可用 —— 无真实消息
    /// 时快照为空，`inject_input` 注入的事件照常折叠（自动化/headless）。
    pub fn collect_input(&mut self) -> InputSnapshot {
        for ev in drain_input() {
            self.input_collector.push(ev);
        }
        let snap = self.input_collector.frame();
        *self.input_state.borrow_mut() = snap.clone();
        snap
    }

    /// 把快照的**边缘**发射成标准 `input/*` 信号（宿主预发纪律：在
    /// `frame*` 之前调用，信号当帧入泵 —— S7.1 黄金帧序）。返回发射数。
    ///
    /// 信号名契约（载荷）：
    /// - `input/key_down` / `input/key_up`（Str 键名，[`Key::name`] 口径）
    /// - `input/mouse_move`（Vec2 位置；仅本帧有位移时）
    /// - `input/mouse_down` / `input/mouse_up`（Str "left"/"right"/"middle"）
    /// - `input/text`（Str 本帧提交的字符 —— 非 UTF-16 合法标量的单元
    ///   被丢弃并如实计数在返回值外不装；一次一条整帧文本）
    /// - `input/window/resized`（Vec2 新客户区尺寸）
    ///
    /// 按住态（轮询）不走信号 —— 脚本用 `key("名")` 探针读。
    pub fn emit_input_signals(&mut self, snap: &InputSnapshot) -> usize {
        let mut n = 0usize;
        let emit = |tree: &mut SceneTree, name: &str, v: Value| {
            tree.emit_signal(name, v);
        };
        for k in &snap.pressed {
            emit(self.tree_mut(), "input/key_down", Value::Str(k.name()));
            n += 1;
        }
        for k in &snap.released {
            emit(self.tree_mut(), "input/key_up", Value::Str(k.name()));
            n += 1;
        }
        if snap.mouse_delta != Vec2::new(0.0, 0.0) {
            emit(
                self.tree_mut(),
                "input/mouse_move",
                Value::Vec2(nes_scene::Vec2::new(snap.mouse.x, snap.mouse.y)),
            );
            n += 1;
        }
        for (i, name) in ["left", "right", "middle"].into_iter().enumerate() {
            if snap.buttons_pressed[i] {
                emit(self.tree_mut(), "input/mouse_down", Value::Str(name.into()));
                n += 1;
            }
            if snap.buttons_released[i] {
                emit(self.tree_mut(), "input/mouse_up", Value::Str(name.into()));
                n += 1;
            }
        }
        if !snap.text.is_empty() {
            let text: String = snap
                .text
                .iter()
                .filter_map(|&c| char::from_u32(c))
                .collect();
            emit(self.tree_mut(), "input/text", Value::Str(text));
            n += 1;
        }
        if let Some((w, h)) = snap.resized {
            emit(
                self.tree_mut(),
                "input/window/resized",
                Value::Vec2(nes_scene::Vec2::new(w as f32, h as f32)),
            );
            n += 1;
        }
        n
    }

    /// 给脚本 VM 接键探针（`key("名")` -> 本帧快照 `is_down`）。
    ///
    /// 装一次即可（共享槽：之后每帧 `collect_input` 自动刷新读数）。
    /// VM 不碰平台 —— 与文件读取器同一注入纪律（S6.33/S7.2）。
    pub fn mount_key_probe(&self, vm: &mut ScriptVm) {
        let state = self.input_state.clone();
        vm.set_key_probe(Rc::new(move |name: &str| state.borrow().is_down(name)));
    }

    /// 轮询文件变化（内容戳判定）。变化后调用 [`Self::upload_pending_textures`] 重传。
    pub fn poll_reloads(&mut self) -> ReloadReport {
        self.registry.poll_reloads()
    }

    /// 把已就绪且**版本变化**的纹理上传到 GPU 注册表，返回本次上传张数。
    ///
    /// 键位衔接：`AssetKey::as_render_key().to_bits()` 与提取层
    /// `render_key_of_bits` 是同一位镜像 —— 场景节点引用的槽位、提取层推出的
    /// `RenderAssetKey`、GPU 注册表里的纹理，三者由同一组位对齐。
    /// 解码失败（非 BMP / 数据截断）如实报错并指名路径，不静默跳过。
    pub fn upload_pending_textures(&mut self) -> Result<usize, BackendError> {
        let mut uploaded = 0;
        let slots: Vec<ResId> = self.texture_slots.clone();
        for id in slots {
            let (key, path_text, version, bytes) = {
                let Some(entry) = self.table.entry(id) else {
                    continue;
                };
                let Some(key) = entry.key() else {
                    continue; // 未绑定（还没 bind）
                };
                let Some(loaded) = self.registry.loaded(key) else {
                    continue; // 未就绪 / 加载失败（状态留在表里，宿主可查询重试）
                };
                let path_text = entry
                    .path()
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| format!("slot {}", id.get()));
                (key, path_text, loaded.version, loaded.bytes.clone())
            };
            if self.uploaded_version.get(&id).copied() == Some(version) {
                continue; // 版本没变（热重载判定）
            }
            let Some(view) = key.as_render_key() else {
                continue; // 非渲染类（本方法只管纹理）
            };
            let (w, h, rgba) = bmp::load_rgba(&bytes)
                .map_err(|e| BackendError::Io(format!("纹理 {path_text} 解码失败：{e}")))?;
            let Some(consumer) = &mut self.consumer else {
                return Err(BackendError::ConfigMismatch(
                    "headless 运行时没有渲染端（纹理无需上传）".into(),
                ));
            };
            consumer.register_texture(
                RenderAssetKey::from_bits(view.to_bits()),
                w,
                h,
                &rgba,
            )?;
            self.uploaded_version.insert(id, version);
            uploaded += 1;
        }
        Ok(uploaded)
    }

    /// 推进一帧（无行为代码）：tick（结构落地 + 生命周期 + 变换冲洗）->
    /// 提取（含命令流生成）-> GPU 消费。
    ///
    /// 与 [`Self::frame_with`] 走同一条 tick 路径 —— 不接观察者时生命周期
    /// 仍照常推进（enter/ready 标志照置），只是没有监听者。
    pub fn frame(&mut self, frame: &FrameInfo) -> Result<FrameOutcome, BackendError> {
        self.frame_with(frame, &mut NoObserver)
    }

    /// 推进一帧（宿主行为经 [`SceneObserver`] 挂入）。
    ///
    /// 帧内阶段序：**tick**（apply_pending -> enter_tree -> ready -> process
    /// -> 变换冲洗）-> 提取 -> 消费。回调里经 [`nes_scene::NodeCtx`] 发出的
    /// `SetLocal`/`SetProp` 命令立即生效（本帧像素可见）；结构变更（`Tree`/
    /// `Spawn`）延迟到下一帧帧首落地 —— 与场景层草案的生命周期语义一致。
    pub fn frame_with(
        &mut self,
        frame: &FrameInfo,
        obs: &mut dyn SceneObserver,
    ) -> Result<FrameOutcome, BackendError> {
        let _steps = self.simulate(frame.delta, obs);
        self.extractor.extract_into(
            &mut self.tree,
            &self.table,
            &mut self.server,
            frame,
            &mut self.commands,
        );
        let Some(consumer) = &mut self.consumer else {
            return Err(BackendError::ConfigMismatch(
                "headless 运行时没有渲染端（用带 GPU 的装配跑帧）".into(),
            ));
        };
        consumer.consume(&self.commands)
    }
}

/// 写一张 32bpp BMP（测试与演示资产生成用；与 `nes-render-wgpu::bmp` 的
/// 解码器互为逆操作）。
pub fn write_bmp_rgba(
    path: &Path,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<(), BackendError> {
    let expected = (width * height * 4) as usize;
    if width == 0 || height == 0 || rgba.len() != expected {
        return Err(BackendError::PixelBufferSize {
            expected,
            actual: rgba.len(),
        });
    }
    let row = (width * 4) as usize;
    let data_size = row * height as usize;
    let mut out = Vec::with_capacity(54 + data_size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(54 + data_size as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes()); // BITMAPINFOHEADER
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(height as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // 平面
    out.extend_from_slice(&32u16.to_le_bytes()); // 位深
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(data_size as u32).to_le_bytes());
    out.extend_from_slice(&[0; 16]);
    // 自底向上行序，RGBA -> BGRA。
    for y in (0..height).rev() {
        for x in 0..width {
            let at = (y * width + x) as usize * 4;
            out.extend_from_slice(&[rgba[at + 2], rgba[at + 1], rgba[at], rgba[at + 3]]);
        }
    }
    std::fs::write(path, &out)
        .map_err(|e| BackendError::Io(format!("写 {} 失败：{e}", path.display())))?;
    Ok(())
}

/// 从消费器借出 GPU 上下文引用（组装层内部用：表面创建需要 &GpuContext）。
fn consumer_ctx(consumer: &CommandConsumer) -> &GpuContext {
    consumer.ctx()
}

/// 已上传纹理数（诊断用）。
pub fn uploaded_texture_count(runtime: &NesRuntime) -> usize {
    runtime.uploaded_version.len()
}

/// 槽位当前绑定的资产键（诊断用；未绑定返回 `None`）。
pub fn asset_key_of(runtime: &NesRuntime, id: ResId) -> Option<AssetKey> {
    runtime.table.entry(id).and_then(|e| e.key())
}
