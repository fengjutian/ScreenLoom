# probes/ —— 探针与留证产物

这里放**开发期的验证材料**，不属于产品代码，也不参与 workspace 构建（根 `Cargo.toml` 的
`members` 里没有它）。

| 子目录 | 内容 | 入库策略 |
|---|---|---|
| `mftprobe/` | 录屏方案（`doc/录屏方案.md`）的 Media Foundation 编码器探针：独立 `[workspace]`，7 个 binary（`probe2`…`probe7`） | 源码入库；`target/`、`out/` 忽略 |
| `evidence/` | 一次性取证产物：`probe_evidence.mp4`（10 秒 H.264，用于 `doc/录屏方案.md` §3.2 数据复核）、`nv12_test.log`、`probe_evidence_check.log` | `.mp4` 入库；`.log` 忽略 |
| `e2e_out/` | E2E 与留证脚本（`scripts/*.ps1`）的输出目录 | 忽略；可随时删除重跑 |

## 跑 MF 探针

```powershell
cargo build --release --manifest-path probes\mftprobe\Cargo.toml
probes\mftprobe\target\release\probe7.exe 1 300 none   # 1=节流开 300=帧数 none=不设 ALL_SAMPLES_INDEPENDENT
```

产物统一落到 `probes/mftprobe/out/`（由 `probes/mftprobe/src/lib.rs` 的 `out_path()` 按
`CARGO_MANIFEST_DIR` 路由，所以挪动目录后不需要改代码），该目录已被 `.gitignore` 忽略。

## 留证截图

`scripts/e2e_*.ps1`、`scripts/capture_*.ps1`、`scripts/probe_tray_*.ps1` 都会把截图写到
`probes/e2e_out/`，目录不存在时脚本自己建。`doc/实现契约.md` 里引用的留证路径都是相对这个目录的。
