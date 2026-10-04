#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""NES 2.0 · M4 依赖方向守卫（S1 出口准则之一）

用法：
    python check_dependency_direction.py [--root <output 目录>] [--json <结果文件>]

检查项（全部基于 `cargo metadata` 解析出的真实依赖图，不做文本猜测）：

    G1  nes-scene 的依赖树（含传递依赖）中不得出现 nes-render-*      —— 架构不倒退
    G2  nes-asset 的依赖树（含传递依赖）中不得出现 nes-render-*      —— 同上（更上游）
    G3  nes-render-api 必须零依赖（normal / dev / build 三类都不得有）
    G4  nes-render-api 不得依赖 nes-scene / nes-asset（契约层不反向绑场景）
    G5  nes-scene / nes-asset 源码中不得出现 nes_render / nes-render 符号（防注释级、feature 级隐性引用）
    G6  nes-render-extract 的直接依赖只能是 nes-scene / nes-render-api（path 依赖），
        且源码中不得直引 nes_asset（资源只能经 RenderKeySource 隔离）
    G7  nes-scene / nes-asset / nes-render-api 均不得（直接或传递）依赖 nes-render-extract

    G6 / G7 为 S2「提取层最小闭环」新增：分层合法性由 G6 正向钉住，
    反向倒灌由 G1 / G2 / G7 三面围堵。

    G8  nes-render-wgpu 的直接依赖只能是 nes-render-api（path 依赖），
        且传递依赖中不得出现 nes-scene / nes-asset / nes-render-extract
        —— 后端是依赖树的叶子，正向钉住
    G9  nes-scene / nes-asset / nes-render-api / nes-render-extract 均不得
        （直接或传递）依赖 nes-render-wgpu —— 反向围堵
    G10 nes-render-wgpu 零第三方依赖（registry 依赖一律越界）、不带 build.rs、
        且是独立工作区根（空 [workspace] 表钉住，不被上层工作区吞并）

    G8 / G9 / G10 为 S4「后端 crate 接入」新增，与 nes-render-wgpu/Cargo.toml
    的依赖纪律注释一一对应。

    G11 nes-runtime（引擎组装层）的直接依赖只能是七个项目 crate（path 依赖），
        且零第三方依赖、独立工作区根；任何 crate 不得依赖 nes-runtime
        —— 组装层是全链顶端叶子，只许被可执行目标（示例/测试）消费

    G12 nes-audio（S13 音频核心）零依赖（normal / dev / build 三类都不得有）、
        无 build.rs、独立工作区根；除 nes-runtime 正向接入（S13 第 2 期，
        runtime ──▶ nes-audio 方向唯一）外，任何 crate 不得（直接或传递）
        依赖它 —— 音频核心是依赖树的纯叶子，正向白名单仅含组装层，反向一律禁止

    G13 nes-media（S14 编解码适配层）是**两个白名单适配层之一（媒体解码 /
        JS 扩展运行时，S17 起）**：其依赖树中的第三方（registry）crate 必须
        全部落在白名单家族内 —— image 系（image 及其全部传递依赖）+
        symphonia 系（symphonia 及其全部传递依赖）；仓库内直接依赖只许
        nes-audio（path）。其它任何 registry 依赖一律越界；除 nes-runtime
        正向接入外，任何 crate 不得依赖 nes-media —— 第三方被收口在最外圈
        的叶子（适配层）上，引擎核心（G3/G10/G12 钉住的那七个）零第三方
        纪律不变。

    G13 为 S14「媒体解码适配层第 1 期」新增：依赖分层政策（用户裁决）
    由本条正向钉住 —— 编解码器采用成熟 Rust 库（image 0.25 系 /
    symphonia 0.5 系），与既有守卫互不侵扰（G1-G12 逐条保持）。
    S17 起 G13 文案修订：nes-media 不再是"全仓库唯一"允许第三方的
    crate —— 依赖分层政策从单叶扩成**双叶**（媒体解码 / JS 扩展运行时），
    见 G15。

    G14 nes-extension-api（S17 扩展 ABI 层）零依赖（normal / dev / build
        三类都不得有）、无 build.rs、独立工作区根 —— 它只定义 NES 自己的
        Extension API（值类型 / JsRuntime trait / 能力 traits / 生命周期），
        任何第三方或仓库内依赖都会泄进冻结面；除 nes-extension-js（绑定
        实现）与 nes-runtime（能力宿主，S17 第 2 期正向接入）外，任何
        crate 不得依赖它 —— **要冻结的是 Extension API，不是某个 JS 引擎**。

    G15 nes-extension-js（S17 JS 扩展运行时）是**两个白名单适配层中的
        另一个**：其依赖树中的第三方（registry）crate 必须全部落在
        **rquickjs 家族闭包**内（rquickjs 及其全部传递依赖，含
        rquickjs-core / rquickjs-sys 与 QuickJS-NG 的 C 编译链）；仓库内
        直接依赖只许 nes-extension-api（path）。其它任何 registry 依赖
        一律越界；除 nes-runtime 正向接入外，任何 crate 不得依赖它 ——
        JS 引擎类型被收口在绑定层的叶子上，上层只见 nes-extension-api
        的冻结面（可换 backend：Boa / quickjs-rs 是未来可选实现方）。

    G14 / G15 为 S17「扩展运行时第 1 期」新增，与 nes-extension-api /
    nes-extension-js 两份 Cargo.toml 的依赖纪律注释一一对应；
    G11 的 runtime 正向白名单同步扩两条（第 2 期接线消费）。

退出码：0 = 全部通过；1 = 有检查项失败；2 = 环境/参数错误（例如 cargo 不可用）。
仅使用 Python 标准库，可直接接入 CI。
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys

FORBIDDEN_PREFIX = "nes-render"
PROTECTED = ("nes-scene", "nes-asset")          # 不得依赖任何渲染 crate
CONTRACT_CRATE = "nes-render-api"               # 必须零依赖
CONTRACT_FORBIDDEN = ("nes-scene", "nes-asset")  # 契约层不得反向依赖
EXTRACT_CRATE = "nes-render-extract"            # 提取层：只许向下依赖场景层 + 契约层
EXTRACT_ALLOWED = ("nes-scene", CONTRACT_CRATE)  # 提取层的全部合法直接依赖
EXTRACT_FORBIDDEN = ("nes-asset",)              # 提取层不得直引资源层（走 RenderKeySource）
NO_DEP_ON_EXTRACT = PROTECTED + (CONTRACT_CRATE,)  # 这些层不得依赖提取层
BACKEND_CRATE = "nes-render-wgpu"                  # GPU 后端：依赖树的叶子
BACKEND_ALLOWED = (CONTRACT_CRATE,)                # 后端唯一合法直接依赖：契约层（path）
BACKEND_FORBIDDEN_UPSTREAM = ("nes-scene", "nes-asset", "nes-render-extract")
NO_DEP_ON_BACKEND = PROTECTED + (CONTRACT_CRATE, EXTRACT_CRATE)  # 这些层不得依赖后端
RUNTIME_CRATE = "nes-runtime"                      # 引擎组装层：全链顶端叶子
MEDIA_CRATE = "nes-media"                          # S14 编解码适配层：两个白名单适配层之一
EXTENSION_API_CRATE = "nes-extension-api"          # S17 扩展 ABI 层：纯冻结面（零依赖）
EXTENSION_JS_CRATE = "nes-extension-js"            # S17 JS 扩展运行时：另一个白名单适配层
# S13 第 2 期：runtime 正向接入音频（runtime ──▶ nes-audio 方向唯一）。
# S14 第 1 期：runtime 正向接入编解码适配层（解码产物 -> 既有纹理/Wav 面）。
# S17 第 2 期：runtime 正向接入扩展运行时（扩展写树 = 游戏状态，经能力
#              traits 回写场景层；JS 引擎类型不出 nes-extension-js）。
RUNTIME_ALLOWED = ("nes-asset", "nes-scene", "nes-render-api",
                   "nes-render-extract", "nes-render-wgpu", "nes-audio",
                   MEDIA_CRATE, EXTENSION_API_CRATE, EXTENSION_JS_CRATE)
SOURCE_FORBIDDEN_RE = re.compile(r"nes[-_]render", re.IGNORECASE)
SOURCE_SCAN_EXT = (".rs", ".toml")
# 提取层源码里不得出现对资源层类型的真实引用（注释里提名字不算，故只匹配 use / 路径限定调用）。
EXTRACT_ASSET_RE = re.compile(
    r"\buse\s+nes_asset\b|\bextern\s+crate\s+nes_asset\b|\bnes_asset\s*::"
)
AUDIO_CRATE = "nes-audio"                          # S13 音频核心：依赖树的纯叶子（wav + 混音 + waveOut）
# 不得依赖 nes-audio 的 crate：S13 第 2 期起 runtime 正向接入（runtime ──▶
# nes-audio 方向唯一，由 G11 的白名单正向钉住），反向禁令不再含 runtime。
# S14 第 1 期起 nes-media 亦正向依赖它（编解码适配层产 nes_audio::Wav
# 同构 DTO）—— 该方向的合法性由 G13 的白名单钉住，不在此清单内。
NO_DEP_ON_AUDIO = PROTECTED + (CONTRACT_CRATE, EXTRACT_CRATE, BACKEND_CRATE)
MEDIA_THIRD_ROOTS = ("image", "symphonia")         # 第三方白名单根（及其全部传递依赖）
MEDIA_REPO_ALLOWED = ("nes-audio",)                # 仓库内白名单（必须 path 依赖）
MEDIA_CONSUMERS_ALLOWED = (RUNTIME_CRATE,)         # 唯一消费方：引擎组装层
# S17 扩展生态：nes-extension-api 的正向白名单（绑定实现 + 能力宿主）与
# nes-extension-js 的第三方家族根（rquickjs 及其传递闭包）/ 仓库内白名单 /
# 唯一消费方（第 2 期接线，先钉政策）。
EXTAPI_REPO_ALLOWED = (EXTENSION_JS_CRATE, RUNTIME_CRATE)
EXTJS_THIRD_ROOTS = ("rquickjs",)                  # rquickjs 家族闭包（含 QuickJS-NG 编译链）
EXTJS_REPO_ALLOWED = (EXTENSION_API_CRATE,)        # 仓库内白名单（必须 path 依赖）
EXTJS_CONSUMERS_ALLOWED = (RUNTIME_CRATE,)         # 唯一消费方：引擎组装层


class Colors:
    OK = "\033[32m"
    FAIL = "\033[31m"
    WARN = "\033[33m"
    DIM = "\033[2m"
    END = "\033[0m"

    @classmethod
    def strip(cls) -> None:
        cls.OK = cls.FAIL = cls.WARN = cls.DIM = cls.END = ""


def run_metadata(manifest: str) -> dict:
    """调用 cargo metadata，返回解析后的 JSON。"""
    cmd = ["cargo", "metadata", "--format-version", "1", "--manifest-path", manifest]
    proc = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace")
    if proc.returncode != 0:
        raise RuntimeError(
            "cargo metadata 失败（%s）：\n%s" % (manifest, (proc.stderr or "").strip()[:2000])
        )
    return json.loads(proc.stdout)


def root_dep_names(meta: dict) -> list:
    """返回根 crate 的**全部传递依赖**包名（不含根自身），按字母序。"""
    nodes = {n["id"]: n for n in meta.get("resolve", {}).get("nodes", [])}
    root_id = meta.get("resolve", {}).get("root")
    if root_id is None or root_id not in nodes:
        # 单包没有 resolve.root 时退化为按 manifest 命中
        for pkg in meta.get("packages", []):
            if os.path.normcase(os.path.abspath(pkg["manifest_path"])) == os.path.normcase(
                os.path.abspath(meta["workspace_root"] + "/Cargo.toml")
            ):
                root_id = pkg["id"]
                break
    if root_id is None:
        return []

    seen, queue = set(), [root_id]
    while queue:
        nid = queue.pop()
        for dep in nodes.get(nid, {}).get("deps", []):
            pid = dep["pkg"]
            if pid in seen or pid == root_id:
                continue
            seen.add(pid)
            queue.append(pid)
    names = {pkg["name"] for pkg in meta.get("packages", []) if pkg["id"] in seen}
    return sorted(names)


def reachable_names_from(meta: dict, start_name: str) -> set:
    """返回依赖图中从指定包名出发可达的**全部包名**（含起点自身）。

    G13 用它圈出白名单家族：`image` / `symphonia` 各自的传递闭包 ——
    白名单根带来的依赖是白名单的内在成本，跟踪家族闭包而不是逐个
    枚举包名，第三方补丁版本换依赖时守卫不需要跟着改。
    """
    nodes = {n["id"]: n for n in meta.get("resolve", {}).get("nodes", [])}
    name_of = {pkg["id"]: pkg["name"] for pkg in meta.get("packages", [])}
    start_ids = [pid for pid, name in name_of.items() if name == start_name]
    seen, queue = set(), list(start_ids)
    while queue:
        nid = queue.pop()
        if nid in seen:
            continue
        seen.add(nid)
        for dep in nodes.get(nid, {}).get("deps", []):
            if dep["pkg"] not in seen:
                queue.append(dep["pkg"])
    return {name_of[pid] for pid in seen if pid in name_of}


def declared_deps(meta: dict, crate: str) -> list:
    """返回根包的**直接声明依赖**（含 dev / build），元素为 (name, kind, path_or_None)。"""
    out = []
    for pkg in meta.get("packages", []):
        if pkg["name"] != crate:
            continue
        for dep in pkg.get("dependencies", []):
            out.append((dep["name"], dep.get("kind") or "normal", dep.get("path")))
    return sorted(out)


def scan_sources(crate_dir: str, pattern: re.Pattern = SOURCE_FORBIDDEN_RE,
                 exts: tuple = SOURCE_SCAN_EXT, strip_comments: bool = False):
    """在 crate 源码/清单中查找命中 `pattern` 的行，返回 (命中列表, 扫描文件数)。

    `strip_comments=True` 时先裁掉 `//` 起及其后的内容，只认真实代码引用
    （G6 用它区分"注释里提到资源层类型"与"真的 use 了资源层"）。
    """
    hits = []
    scanned = 0
    for base, dirs, files in os.walk(crate_dir):
        dirs[:] = [d for d in dirs if d not in {"target", ".git"}]
        for name in files:
            if not name.endswith(exts):
                continue
            scanned += 1
            full = os.path.join(base, name)
            try:
                with open(full, "r", encoding="utf-8", errors="replace") as fh:
                    for idx, line in enumerate(fh, 1):
                        probe = line.split("//", 1)[0] if strip_comments else line
                        if pattern.search(probe):
                            hits.append((os.path.relpath(full, crate_dir), idx, line.strip()))
            except OSError:
                continue
    return hits, scanned


def main() -> int:
    parser = argparse.ArgumentParser(description="M4 依赖方向守卫（nes-render-* 不得倒灌进场景层）")
    parser.add_argument("--root", default=os.path.dirname(os.path.abspath(__file__)),
                        help="包含各 crate 的目录，默认脚本所在目录")
    parser.add_argument("--json", default=None, help="把结果同时写入该 JSON 文件")
    parser.add_argument("--no-color", action="store_true", help="关闭彩色输出（CI 友好）")
    args = parser.parse_args()

    if args.no_color or not sys.stdout.isatty():
        Colors.strip()

    root = os.path.abspath(args.root)
    if shutil.which("cargo") is None:
        print("cargo 不在 PATH 中，无法执行依赖方向检查", file=sys.stderr)
        return 2

    crates = PROTECTED + (CONTRACT_CRATE, EXTRACT_CRATE, BACKEND_CRATE, RUNTIME_CRATE,
                          AUDIO_CRATE, MEDIA_CRATE, EXTENSION_API_CRATE, EXTENSION_JS_CRATE)
    missing = [c for c in crates if not os.path.isfile(os.path.join(root, c, "Cargo.toml"))]
    if missing:
        print("缺少 crate 清单文件：%s" % ", ".join(missing), file=sys.stderr)
        return 2

    results = []

    def record(check_id: str, title: str, ok: bool, detail: str) -> None:
        results.append({"id": check_id, "title": title, "ok": bool(ok), "detail": detail})
        flag = (Colors.OK + "PASS" + Colors.END) if ok else (Colors.FAIL + "FAIL" + Colors.END)
        print("[%s] %s %s" % (flag, check_id, title))
        for line in detail.splitlines() or [""]:
            print("        %s%s%s" % (Colors.DIM, line, Colors.END))

    try:
        metas = {
            c: run_metadata(os.path.join(root, c, "Cargo.toml")) for c in crates
        }
    except RuntimeError as exc:
        print(str(exc), file=sys.stderr)
        return 2

    # ---- G1 / G2：场景层与资源层不得依赖任何 nes-render-*（含传递依赖）----
    for crate in PROTECTED:
        deps = root_dep_names(metas[crate])
        bad = [d for d in deps if d.startswith(FORBIDDEN_PREFIX)]
        detail = "传递依赖 %d 个：%s" % (len(deps), ", ".join(deps) if deps else "(无)")
        if bad:
            detail += "\n违规依赖：" + ", ".join(bad)
        record("G1" if crate == PROTECTED[0] else "G2",
               "%s 的依赖树中不含 nes-render-*" % crate, not bad, detail)

    # ---- G3：契约层零依赖 ----
    declared = declared_deps(metas[CONTRACT_CRATE], CONTRACT_CRATE)
    detail = "声明依赖 %d 条：%s" % (
        len(declared),
        ", ".join("%s[%s]" % (n, k) for n, k, _ in declared) if declared else "(无)",
    )
    if declared:
        detail += "\n契约层出现依赖即视为冻结失效（零依赖是 S1 的硬约束）"
    record("G3", "%s 零依赖（normal/dev/build 均为空）" % CONTRACT_CRATE, not declared, detail)

    # ---- G4：契约层不得反向依赖场景层/资源层 ----
    trans = root_dep_names(metas[CONTRACT_CRATE])
    bad = [d for d in trans if d in CONTRACT_FORBIDDEN]
    detail = "传递依赖 %d 个：%s" % (len(trans), ", ".join(trans) if trans else "(无)")
    if bad:
        detail += "\n违规依赖：" + ", ".join(bad)
    record("G4", "%s 不依赖 %s" % (CONTRACT_CRATE, " / ".join(CONTRACT_FORBIDDEN)), not bad, detail)

    # ---- G5：源码级隐性引用（注释、feature 门、字符串）----
    all_hits = []
    scanned_total = 0
    for crate in PROTECTED:
        hits, scanned = scan_sources(os.path.join(root, crate))
        scanned_total += scanned
        for rel, line_no, line in hits:
            all_hits.append("%s/%s:%d: %s" % (crate, rel, line_no, line[:160]))
    detail = "扫描 .rs/.toml 文件 %d 个，命中 %d 处" % (scanned_total, len(all_hits))
    if all_hits:
        detail += "\n" + "\n".join(all_hits[:20])
    record("G5", "场景层/资源层源码中不出现 nes_render 符号", not all_hits, detail)

    # ---- G6：提取层只许向下依赖 nes-scene / nes-render-api（且必须是 path 依赖）----
    ex_declared = declared_deps(metas[EXTRACT_CRATE], EXTRACT_CRATE)
    ex_trans = root_dep_names(metas[EXTRACT_CRATE])
    illegal = [n for n, _k, _p in ex_declared if n not in EXTRACT_ALLOWED]
    unpathed = [n for n, _k, p in ex_declared if n in EXTRACT_ALLOWED and not p]
    # 资源隔离只约束**直接依赖**：nes-asset 经 nes-scene 传递进来是合法且必然的，
    # 真正要防的是提取层直接 `use nes_asset` 绕开 RenderKeySource。
    asset_direct = [n for n, _k, _p in ex_declared if n in EXTRACT_FORBIDDEN]
    asset_hits, asset_scanned = scan_sources(
        os.path.join(root, EXTRACT_CRATE), EXTRACT_ASSET_RE, (".rs",), strip_comments=True
    )
    detail = "声明依赖 %d 条：%s" % (
        len(ex_declared),
        ", ".join("%s[%s]%s" % (n, k, "(path)" if p else "(registry)") for n, k, p in ex_declared)
        or "(无)",
    )
    detail += "\n传递依赖 %d 个：%s" % (len(ex_trans), ", ".join(ex_trans) if ex_trans else "(无)")
    detail += "\n合法直接依赖白名单：%s（ne-asset 不在其中）" % ", ".join(EXTRACT_ALLOWED)
    detail += "\n源码扫描 .rs 文件 %d 个，直接引用 nes_asset 命中 %d 处" % (
        asset_scanned,
        len(asset_hits),
    )
    if illegal:
        detail += "\n越界依赖（提取层不得引入）：" + ", ".join(illegal)
    if unpathed:
        detail += "\n白名单内但非 path 依赖：" + ", ".join(unpathed)
    if asset_direct:
        detail += "\n资源层直接依赖（必须走 RenderKeySource 隔离）：" + ", ".join(asset_direct)
    for rel, line_no, line in asset_hits[:10]:
        detail += "\n  %s:%d: %s" % (rel, line_no, line[:160])
    record(
        "G6",
        "%s 直接依赖仅限 %s（path），且源码不直引 nes-asset"
        % (EXTRACT_CRATE, " / ".join(EXTRACT_ALLOWED)),
        not (illegal or unpathed or asset_direct or asset_hits),
        detail,
    )

    # ---- G7：场景层/资源层/契约层不得反向依赖提取层 ----
    rev_bad = []
    rev_lines = []
    for crate in NO_DEP_ON_EXTRACT:
        deps = root_dep_names(metas[crate])
        rev_lines.append("%s：传递依赖 %d 个" % (crate, len(deps)))
        for d in deps:
            if d == EXTRACT_CRATE:
                rev_bad.append("%s -> %s" % (crate, d))
    detail = "\n".join(rev_lines)
    if rev_bad:
        detail += "\n违规反向依赖：" + ", ".join(rev_bad)
    record(
        "G7",
        "%s 均不（直接或传递）依赖 %s" % (" / ".join(NO_DEP_ON_EXTRACT), EXTRACT_CRATE),
        not rev_bad,
        detail,
    )

    # ---- G8：后端 crate 是依赖树的叶子（直接依赖仅限契约层，传递依赖不见上游）----
    be_declared = declared_deps(metas[BACKEND_CRATE], BACKEND_CRATE)
    be_trans = root_dep_names(metas[BACKEND_CRATE])
    be_illegal = [n for n, _k, _p in be_declared if n not in BACKEND_ALLOWED]
    be_unpathed = [n for n, _k, p in be_declared if n in BACKEND_ALLOWED and not p]
    be_upstream = [d for d in be_trans if d in BACKEND_FORBIDDEN_UPSTREAM]
    detail = "声明依赖 %d 条：%s" % (
        len(be_declared),
        ", ".join("%s[%s]%s" % (n, k, "(path)" if p else "(registry)") for n, k, p in be_declared)
        or "(无)",
    )
    detail += "\n传递依赖 %d 个：%s" % (len(be_trans), ", ".join(be_trans) if be_trans else "(无)")
    detail += "\n合法直接依赖白名单：%s（场景/资源/提取层均不在其中）" % ", ".join(BACKEND_ALLOWED)
    if be_illegal:
        detail += "\n越界依赖（后端是叶子，只许依赖契约层）：" + ", ".join(be_illegal)
    if be_unpathed:
        detail += "\n白名单内但非 path 依赖：" + ", ".join(be_unpathed)
    if be_upstream:
        detail += "\n传递依赖中出现上游层：" + ", ".join(be_upstream)
    record(
        "G8",
        "%s 直接依赖仅限 %s（path），传递依赖不含 %s"
        % (BACKEND_CRATE, " / ".join(BACKEND_ALLOWED), " / ".join(BACKEND_FORBIDDEN_UPSTREAM)),
        not (be_illegal or be_unpathed or be_upstream),
        detail,
    )

    # ---- G9：任何上游 crate 不得（直接或传递）反向依赖后端 ----
    be_rev_bad = []
    be_rev_lines = []
    for crate in NO_DEP_ON_BACKEND:
        deps = root_dep_names(metas[crate])
        be_rev_lines.append("%s：传递依赖 %d 个" % (crate, len(deps)))
        for d in deps:
            if d == BACKEND_CRATE:
                be_rev_bad.append("%s -> %s" % (crate, d))
    detail = "\n".join(be_rev_lines)
    if be_rev_bad:
        detail += "\n违规反向依赖：" + ", ".join(be_rev_bad)
    record(
        "G9",
        "%s 均不（直接或传递）依赖 %s" % (" / ".join(NO_DEP_ON_BACKEND), BACKEND_CRATE),
        not be_rev_bad,
        detail,
    )

    # ---- G10：后端零第三方依赖、无 build.rs、独立工作区根 ----
    be_registry = [n for n, _k, p in be_declared if not p]
    be_build_rs = os.path.isfile(os.path.join(root, BACKEND_CRATE, "build.rs"))
    be_ws_root = os.path.normcase(
        os.path.abspath(metas[BACKEND_CRATE].get("workspace_root", ""))
    )
    be_own_ws = be_ws_root == os.path.normcase(os.path.join(root, BACKEND_CRATE))
    detail = "registry（非 path）依赖 %d 条：%s" % (
        len(be_registry),
        ", ".join(be_registry) if be_registry else "(无)",
    )
    detail += "\nbuild.rs 存在：%s（应为否：FFI 绑定手写，不需要 build script）" % (
        "是" if be_build_rs else "否"
    )
    detail += "\nworkspace_root：%s（%s）" % (
        metas[BACKEND_CRATE].get("workspace_root", "(未知)"),
        "独立工作区根" if be_own_ws else "被上层工作区吞并",
    )
    if be_registry:
        detail += "\n越界的第三方 crate：" + ", ".join(be_registry)
    if be_build_rs:
        detail += "\n出现 build.rs：本 crate 的纪律是手写 #[repr(C)] + 运行时符号解析"
    if not be_own_ws:
        detail += "\n后端被并入上层工作区：GPU 侧依赖面会污染场景层构建图，须以空 [workspace] 表钉回独立根"
    record(
        "G10",
        "%s 零第三方依赖（registry 一律越界）、无 build.rs、独立工作区根" % BACKEND_CRATE,
        not (be_registry or be_build_rs or not be_own_ws),
        detail,
    )

    # ---- G11：引擎组装层是全链顶端叶子（只许向下依赖五个项目 crate）----
    rt_declared = declared_deps(metas[RUNTIME_CRATE], RUNTIME_CRATE)
    rt_trans = root_dep_names(metas[RUNTIME_CRATE])
    rt_illegal = [n for n, _k, _p in rt_declared if n not in RUNTIME_ALLOWED]
    rt_unpathed = [n for n, _k, p in rt_declared if n in RUNTIME_ALLOWED and not p]
    rt_registry = [n for n, _k, p in rt_declared if not p]
    rt_ws_root = os.path.normcase(
        os.path.abspath(metas[RUNTIME_CRATE].get("workspace_root", ""))
    )
    rt_own_ws = rt_ws_root == os.path.normcase(os.path.join(root, RUNTIME_CRATE))
    rt_rev_bad = []
    rt_rev_lines = []
    for crate in RUNTIME_ALLOWED:
        deps = root_dep_names(metas[crate])
        rt_rev_lines.append("%s：传递依赖 %d 个" % (crate, len(deps)))
        for d in deps:
            if d == RUNTIME_CRATE:
                rt_rev_bad.append("%s -> %s" % (crate, d))
    detail = "声明依赖 %d 条：%s" % (
        len(rt_declared),
        ", ".join("%s[%s]%s" % (n, k, "(path)" if p else "(registry)") for n, k, p in rt_declared)
        or "(无)",
    )
    detail += "\n传递依赖 %d 个：%s" % (len(rt_trans), ", ".join(rt_trans))
    detail += "\n合法直接依赖白名单：七个项目 crate（全部 path；含 S13 第 2 期正向接入的 nes-audio、S14 第 1 期正向接入的 nes-media）"
    detail += "\n" + "\n".join(rt_rev_lines)
    if rt_illegal:
        detail += "\n越界依赖：" + ", ".join(rt_illegal)
    if rt_registry:
        detail += "\n第三方（非 path）依赖：" + ", ".join(rt_registry)
    if not rt_own_ws:
        detail += "\n组装层未钉成独立工作区根"
    if rt_rev_bad:
        detail += "\n违规反向依赖：" + ", ".join(rt_rev_bad)
    record(
        "G11",
        "%s 仅向下依赖七个项目 crate（path）、零第三方、独立工作区根、无人反向依赖"
        % RUNTIME_CRATE,
        not (rt_illegal or rt_unpathed or rt_registry or not rt_own_ws or rt_rev_bad),
        detail,
    )

    # ---- G12：nes-audio 是依赖树的纯叶子（零依赖、无 build.rs、独立根、
    #      除 runtime 正向接入外无人依赖它）----
    au_declared = declared_deps(metas[AUDIO_CRATE], AUDIO_CRATE)
    au_trans = root_dep_names(metas[AUDIO_CRATE])
    au_registry = [n for n, _k, p in au_declared if not p]
    au_build_rs = os.path.isfile(os.path.join(root, AUDIO_CRATE, "build.rs"))
    au_ws_root = os.path.normcase(
        os.path.abspath(metas[AUDIO_CRATE].get("workspace_root", ""))
    )
    au_own_ws = au_ws_root == os.path.normcase(os.path.join(root, AUDIO_CRATE))
    au_rev_bad = []
    au_rev_lines = []
    for crate in NO_DEP_ON_AUDIO:
        deps = root_dep_names(metas[crate])
        au_rev_lines.append("%s：传递依赖 %d 个" % (crate, len(deps)))
        for d in deps:
            if d == AUDIO_CRATE:
                au_rev_bad.append("%s -> %s" % (crate, d))
    detail = "声明依赖 %d 条：%s" % (
        len(au_declared),
        ", ".join("%s[%s]" % (n, k) for n, k, _ in au_declared) if au_declared else "(无)",
    )
    detail += "\n传递依赖 %d 个：%s" % (len(au_trans), ", ".join(au_trans) if au_trans else "(无)")
    detail += "\nregistry（非 path）依赖 %d 条：%s" % (
        len(au_registry),
        ", ".join(au_registry) if au_registry else "(无)",
    )
    detail += "\nbuild.rs 存在：%s（应为否：WAV 手写解析、winmm 绑定手写 #[repr(C)]）" % (
        "是" if au_build_rs else "否"
    )
    detail += "\nworkspace_root：%s（%s）" % (
        metas[AUDIO_CRATE].get("workspace_root", "(未知)"),
        "独立工作区根" if au_own_ws else "被上层工作区吞并",
    )
    detail += "\n" + "\n".join(au_rev_lines)
    if au_declared:
        detail += "\n出现依赖即越界：音频核心 P0 是零依赖纯叶子（第 2 期 runtime 正向接入时另行放宽）"
    if au_build_rs:
        detail += "\n出现 build.rs：本 crate 的纪律是手写解析 + 手写 FFI，不需要构建脚本"
    if not au_own_ws:
        detail += "\n音频核心未钉成独立工作区根：须以空 [workspace] 表钉回独立根"
    if au_rev_bad:
        detail += "\n违规反向依赖：" + ", ".join(au_rev_bad)
    record(
        "G12",
        "%s 零依赖（normal/dev/build 均为空）、无 build.rs、独立工作区根、除 runtime 正向接入外无人反向依赖"
        % AUDIO_CRATE,
        not (au_declared or au_registry or au_build_rs or not au_own_ws or au_rev_bad),
        detail,
    )

    # ---- G13：nes-media 是两个白名单适配层之一（媒体解码，S14 依赖分层政策）----
    #      第三方（registry）依赖树必须全部落在 image 系 / symphonia 系两个
    #      白名单家族的传递闭包内；仓库内直接依赖只许 nes-audio（path）；
    #      除 nes-runtime 正向接入外无人依赖它。引擎核心的零第三方纪律
    #      （G3/G10/G11/G12/G14）不受影响 —— 第三方收口在最外圈的两片叶子
    #      上（媒体解码 / JS 扩展运行时，后者见 G15）。
    md_declared = declared_deps(metas[MEDIA_CRATE], MEDIA_CRATE)
    md_trans = root_dep_names(metas[MEDIA_CRATE])
    allowed_third: set = set()
    for family_root in MEDIA_THIRD_ROOTS:
        allowed_third |= reachable_names_from(metas[MEDIA_CRATE], family_root)
    # 家族闭包只该圈住第三方 —— 仓库内 crate 若混进闭包（白名单根路径依赖
    # 了仓库 crate），按"仓库依赖单列"的口径剔除，由下面的 repo 规则管辖。
    md_illegal = [
        d for d in md_trans
        if d not in allowed_third and d not in MEDIA_REPO_ALLOWED
    ]
    md_repo = [n for n, _k, p in md_declared if p]
    md_repo_bad = [n for n in md_repo if n not in MEDIA_REPO_ALLOWED]
    md_unpathed = [
        n for n, _k, p in md_declared if not p and n not in allowed_third
    ]
    md_rev_bad = []
    md_rev_lines = []
    for crate in PROTECTED + (CONTRACT_CRATE, EXTRACT_CRATE, BACKEND_CRATE,
                              AUDIO_CRATE, MEDIA_CRATE,
                              EXTENSION_API_CRATE, EXTENSION_JS_CRATE):
        deps = root_dep_names(metas[crate])
        md_rev_lines.append("%s：传递依赖 %d 个" % (crate, len(deps)))
        for d in deps:
            if d == MEDIA_CRATE:
                md_rev_bad.append("%s -> %s" % (crate, d))
    detail = "声明依赖 %d 条：%s" % (
        len(md_declared),
        ", ".join("%s[%s]%s" % (n, k, "(path)" if p else "(registry)") for n, k, p in md_declared)
        or "(无)",
    )
    detail += "\n传递依赖 %d 个：%s" % (len(md_trans), ", ".join(md_trans) or "(无)")
    detail += "\n第三方白名单（传递闭包 %d 包）：%s" % (
        len(allowed_third), ", ".join(sorted(allowed_third)))
    detail += "\n仓库内白名单：%s（必须 path）" % ", ".join(MEDIA_REPO_ALLOWED)
    detail += "\n" + "\n".join(md_rev_lines)
    if md_illegal:
        detail += "\n越界的第三方依赖（白名单外）：" + ", ".join(md_illegal)
    if md_repo_bad:
        detail += "\n越界的仓库内依赖（只许 nes-audio）：" + ", ".join(md_repo_bad)
    if md_unpathed:
        detail += "\n非 path 且不在任何白名单家族内的直接依赖：" + ", ".join(md_unpathed)
    if md_rev_bad:
        detail += "\n违规反向依赖（唯一合法消费方是 %s）：%s" % (
            " / ".join(MEDIA_CONSUMERS_ALLOWED), ", ".join(md_rev_bad))
    record(
        "G13",
        "%s 第三方依赖全部落在 image/symphonia 白名单家族内、仓库内仅 nes-audio（path）、"
        "除 runtime 正向接入外无人反向依赖" % MEDIA_CRATE,
        not (md_illegal or md_repo_bad or md_unpathed or md_rev_bad),
        detail,
    )

    # ---- G14：nes-extension-api 是纯 ABI 冻结面（零依赖、无 build.rs、
    #      独立根；除绑定实现与能力宿主外无人依赖它）----
    ea_declared = declared_deps(metas[EXTENSION_API_CRATE], EXTENSION_API_CRATE)
    ea_trans = root_dep_names(metas[EXTENSION_API_CRATE])
    ea_build_rs = os.path.isfile(os.path.join(root, EXTENSION_API_CRATE, "build.rs"))
    ea_ws_root = os.path.normcase(
        os.path.abspath(metas[EXTENSION_API_CRATE].get("workspace_root", ""))
    )
    ea_own_ws = ea_ws_root == os.path.normcase(os.path.join(root, EXTENSION_API_CRATE))
    ea_rev_bad = []
    ea_rev_lines = []
    # 反向禁令不含 nes-extension-js（绑定实现）与 nes-runtime（能力宿主，
    # 第 2 期接线）—— 二者是 EXTAPI_REPO_ALLOWED 的正向白名单。
    for crate in PROTECTED + (CONTRACT_CRATE, EXTRACT_CRATE, BACKEND_CRATE,
                              AUDIO_CRATE, MEDIA_CRATE):
        deps = root_dep_names(metas[crate])
        ea_rev_lines.append("%s：传递依赖 %d 个" % (crate, len(deps)))
        for d in deps:
            if d == EXTENSION_API_CRATE:
                ea_rev_bad.append("%s -> %s" % (crate, d))
    detail = "声明依赖 %d 条：%s" % (
        len(ea_declared),
        ", ".join("%s[%s]" % (n, k) for n, k, _ in ea_declared) if ea_declared else "(无)",
    )
    detail += "\n传递依赖 %d 个：%s" % (len(ea_trans), ", ".join(ea_trans) if ea_trans else "(无)")
    detail += "\nbuild.rs 存在：%s（应为否：纯接口 + 值类型，无任何构建脚本）" % (
        "是" if ea_build_rs else "否"
    )
    detail += "\nworkspace_root：%s（%s）" % (
        metas[EXTENSION_API_CRATE].get("workspace_root", "(未知)"),
        "独立工作区根" if ea_own_ws else "被上层工作区吞并",
    )
    detail += "\n" + "\n".join(ea_rev_lines)
    if ea_declared:
        detail += "\n出现依赖即越界：ABI 冻结面零依赖（任何第三方/仓库内类型泄入即冻结失效）"
    if ea_build_rs:
        detail += "\n出现 build.rs：本 crate 的纪律是纯接口 + 值类型，不需要构建脚本"
    if not ea_own_ws:
        detail += "\n扩展 ABI 层未钉成独立工作区根：须以空 [workspace] 表钉回独立根"
    if ea_rev_bad:
        detail += "\n违规反向依赖（合法消费方仅 %s）：%s" % (
            " / ".join(EXTAPI_REPO_ALLOWED), ", ".join(ea_rev_bad))
    record(
        "G14",
        "%s 零依赖（normal/dev/build 均为空）、无 build.rs、独立工作区根、"
        "除绑定实现/能力宿主外无人反向依赖" % EXTENSION_API_CRATE,
        not (ea_declared or ea_build_rs or not ea_own_ws or ea_rev_bad),
        detail,
    )

    # ---- G15：nes-extension-js 是另一个白名单适配层（JS 扩展运行时）----
    #      第三方（registry）依赖树必须全部落在 rquickjs 家族闭包内
    #      （rquickjs 及其全部传递依赖，含 QuickJS-NG 的 C 编译链）；
    #      仓库内直接依赖只许 nes-extension-api（path）；除 nes-runtime
    #      正向接入外无人依赖它。JS 引擎类型收口在绑定层叶子上。
    ej_declared = declared_deps(metas[EXTENSION_JS_CRATE], EXTENSION_JS_CRATE)
    ej_trans = root_dep_names(metas[EXTENSION_JS_CRATE])
    ej_build_rs = os.path.isfile(os.path.join(root, EXTENSION_JS_CRATE, "build.rs"))
    ej_ws_root = os.path.normcase(
        os.path.abspath(metas[EXTENSION_JS_CRATE].get("workspace_root", ""))
    )
    ej_own_ws = ej_ws_root == os.path.normcase(os.path.join(root, EXTENSION_JS_CRATE))
    ej_allowed_third: set = set()
    for family_root in EXTJS_THIRD_ROOTS:
        ej_allowed_third |= reachable_names_from(metas[EXTENSION_JS_CRATE], family_root)
    ej_illegal = [
        d for d in ej_trans
        if d not in ej_allowed_third and d not in EXTJS_REPO_ALLOWED
    ]
    ej_repo = [n for n, _k, p in ej_declared if p]
    ej_repo_bad = [n for n in ej_repo if n not in EXTJS_REPO_ALLOWED]
    ej_unpathed = [
        n for n, _k, p in ej_declared if not p and n not in ej_allowed_third
    ]
    ej_rev_bad = []
    ej_rev_lines = []
    for crate in PROTECTED + (CONTRACT_CRATE, EXTRACT_CRATE, BACKEND_CRATE,
                              AUDIO_CRATE, MEDIA_CRATE, EXTENSION_API_CRATE):
        deps = root_dep_names(metas[crate])
        ej_rev_lines.append("%s：传递依赖 %d 个" % (crate, len(deps)))
        for d in deps:
            if d == EXTENSION_JS_CRATE:
                ej_rev_bad.append("%s -> %s" % (crate, d))
    detail = "声明依赖 %d 条：%s" % (
        len(ej_declared),
        ", ".join("%s[%s]%s" % (n, k, "(path)" if p else "(registry)") for n, k, p in ej_declared)
        or "(无)",
    )
    detail += "\n传递依赖 %d 个：%s" % (len(ej_trans), ", ".join(ej_trans) or "(无)")
    detail += "\n第三方白名单（rquickjs 家族闭包 %d 包）：%s" % (
        len(ej_allowed_third), ", ".join(sorted(ej_allowed_third)))
    detail += "\n仓库内白名单：%s（必须 path）" % ", ".join(EXTJS_REPO_ALLOWED)
    detail += "\nbuild.rs 存在：%s（应为否：QuickJS 的 C 编译脚本属 rquickjs-sys，不在本 crate）" % (
        "是" if ej_build_rs else "否"
    )
    detail += "\nworkspace_root：%s（%s）" % (
        metas[EXTENSION_JS_CRATE].get("workspace_root", "(未知)"),
        "独立工作区根" if ej_own_ws else "被上层工作区吞并",
    )
    detail += "\n" + "\n".join(ej_rev_lines)
    if ej_illegal:
        detail += "\n越界的第三方依赖（白名单外）：" + ", ".join(ej_illegal)
    if ej_repo_bad:
        detail += "\n越界的仓库内依赖（只许 nes-extension-api）：" + ", ".join(ej_repo_bad)
    if ej_unpathed:
        detail += "\n非 path 且不在 rquickjs 家族闭包内的直接依赖：" + ", ".join(ej_unpathed)
    if ej_build_rs:
        detail += "\n出现 build.rs：C 编译由 rquickjs-sys 自带构建脚本承担，本 crate 不该有"
    if not ej_own_ws:
        detail += "\nJS 扩展运行时未钉成独立工作区根：须以空 [workspace] 表钉回独立根"
    if ej_rev_bad:
        detail += "\n违规反向依赖（唯一合法消费方是 %s）：%s" % (
            " / ".join(EXTJS_CONSUMERS_ALLOWED), ", ".join(ej_rev_bad))
    record(
        "G15",
        "%s 第三方依赖全部落在 rquickjs 白名单家族内、仓库内仅 nes-extension-api（path）、"
        "除 runtime 正向接入外无人反向依赖" % EXTENSION_JS_CRATE,
        not (ej_illegal or ej_repo_bad or ej_unpathed or ej_build_rs or not ej_own_ws or ej_rev_bad),
        detail,
    )

    passed = sum(1 for r in results if r["ok"])
    total = len(results)
    print()
    print("=" * 64)
    print("依赖方向守卫结果：%d/%d 通过（根目录：%s）" % (passed, total, root))
    print("=" * 64)

    if args.json:
        with open(args.json, "w", encoding="utf-8") as fh:
            json.dump({"root": root, "passed": passed, "total": total, "checks": results},
                      fh, ensure_ascii=False, indent=2)
        print("结果已写入：%s" % args.json)

    return 0 if passed == total else 1


if __name__ == "__main__":
    sys.exit(main())
