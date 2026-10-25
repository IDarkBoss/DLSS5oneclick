# DLSS5oneclick — 工作区说明

Rust + eframe/egui 单 crate、仅面向 Windows 的 GUI 工具：为不带 DLSS 的 DX11/DX12 游戏一键安装泄露版 DLSS 5 神经渲染（ReShade + RenoDX DLSS 5 add-on / DLSS5-Feeder / OptiScaler_DLSSNR fork / MFG 解锁等多条路线）。上游：https://github.com/faisalkindi/DLSS5oneclick 。用户可见文本走 rust-i18n（en / zh-CN 双语），源码里用 `t!("模块.key")` 取文案，翻译在 `locales/*.yml`。

## 常用命令

- 构建：`cargo build`；发布：`cargo build --release`（产物 `target\release\dlss5oneclick.exe`）
- Lint：`cargo clippy --all-targets -- -D warnings`（CI 把警告当错误，本地提交前必须同样通过）
- 测试：`cargo test`（约 217 个单元测试，内联在各模块 `#[cfg(test)]`）
- 运行：`cargo run` 无参启动 GUI；带参数为 headless CLI，完整旗标列表见 `src/main.rs:28` 的 doc 注释（`--check` / `--remove` / `--diagnose` / `--report` / `--engine=` 等）
- CI（`.github/workflows/ci.yml`）在 windows-latest 跑 clippy + test；推 tag `v*` 触发 release 发布

## 模块地图（src/，全部在 main.rs 声明）

- `main.rs` — 入口、headless CLI 解析、panic handler、控制台附加
- `gui.rs` — 全部 egui 界面（"仪表盘"布局），最大的 UI 文件
- `installer.rs` — 安装/更新/移除的实际文件操作（按 DLSS5-Feeder README 的步骤顺序）
- `setup.rs` — "阶梯"逻辑：每个游戏按序回退的安装路线
- `game.rs` — 游戏文件夹检查（exe 位数、DX11/12 检测、已装组件）
- `library.rs` — Steam/Epic/GOG/Xbox/EA/Ubisoft 游戏库扫描
- `diagnose.rs` — 读 ReShade/feed 日志，判断神经渲染为何没在跑
- `net.rs` — GitHub release 页面解析（走页面不走 API，避开限流）、流式下载、zip 解压
- `theme.rs` — 配色、字体、控件样式；`text.rs` — `tidy()` 折叠显示文本的空白
- `i18n.rs` — rust-i18n 封装：语言常量、系统语言检测、`--lang` 旗标与 settings 初始化、key 一致性测试
- 其余：`feeder_cfg` / `gpu` / `gpupref` / `ngx` / `pcgw` / `quality_preset` / `renodx` / `report` / `reshade_ini` / `settings` / `update`

## 关键约定与陷阱

- **显示给用户的长消息都过 `text::tidy()`**：消息写成跨行的字符串字面量，显示时折叠空白（cargo fmt 会把换行变成字符串内的一串空格）；写入文件的文本不要 tidy。
- **代码注释里的 issue 编号**（如 `#95`、`#118`）指上游 GitHub issue，是相应行为存在的原因，改相关代码时保留。
- **build.rs 的两个链接导出不可删**：`NvOptimusEnablement` / `AmdPowerXpressRequestHighPerformance`——混合 GPU 机器上强制进程用独显，没有它们窗口可能死在核显的 OpenGL 驱动里（#32、#23）。
- release 构建 `windows_subsystem = "windows"`（无控制台窗口）；headless 输出靠 `attach_parent_console()`。
- 用户级数据硬编码在 `%LOCALAPPDATA%\dlss5oneclick\`：`settings.json`、`library-cache.json`、`pcgw-cache.json`、`downloads\`。
- exe 内嵌的第三方内容只有 `assets/fonts/` 下五个 SIL-OFL 字体；其余组件全部在安装时从上游 GitHub release 下载。

## i18n（rust-i18n 4，en / zh-CN 双语）

用户可见文本一律 `t!("模块.key")`（宏在 main.rs `#[macro_use] extern crate rust_i18n;` 后全 crate 可用，返回 `Cow<'static, str>`）；翻译在 `locales/*.yml`，`i18n!("locales", fallback = "en")` 编译期嵌入 exe——发布单文件不带翻译目录。

- **YAML 格式**：`_version: 2`、扁平点分 key（禁止嵌套）、每 key 下 `en:` 在前 `zh-CN:` 在后；key 前缀 = 模块名（`installer.` / `gui.` / `game.` …），跨文件不重复。`src/i18n.rs` 的测试 `every_key_exists_in_both_en_and_zh_cn` 强制两语言 key 集合一致、值非空。**没有文案的模块不要建空骨架 yml**（只有 `_version` 的文件会让 `i18n!` 宏编译期 panic）。
- **插值**：`t!("net.err_http", code = status.as_u16())` + YAML `"HTTP %{code}"`；Path 先 `.display().to_string()`；对齐填充等复杂格式保留 `format!` 外壳。
- **语言选择**：`--lang=en|zh-CN` 旗标 > `settings.json` 的 `language`（设置页下拉写入）> Windows 界面语言（中文系统→zh-CN，其余→en）。缺省与缺失 key 回退都是 en。`rust_i18n::set_locale` 是全局原子量，GUI 切换下一帧全界面生效，后台线程的 `t!()` 同样取到。
- **测试约定**：单测**禁止** `set_locale`（全局态污染并行测试）；断言文案一律断言英文（en 既是默认又是回退，双保险）。
- **不进翻译表的内容**：写入游戏文件夹的 ini 键值与节名、文件名、`*.dlss5oneclick` marker 文件、`diagnose.rs` 匹配上游日志的关键字（如 `feature ready`、`0xBAD00001`，那是解析输入不是 UI 文案）、CLI 旗标名、注册表路径；`src/i18n.rs` 里 `--lang` 的双语报错也是刻意的（locale 未定时使用）。
- **新增语言**：`i18n.rs` 的 `LOCALES`/`NATIVE_NAMES` 加一项，每个 `locales/*.yml` 的 key 下补该语言一行（一致性测试会强制补全），`gui.rs` 设置页下拉自动出现。
- 发布文案在 `README.md` 与 `nexus/description.bbcode`（不走 t!）。

### 字体（中文渲染）

内置字体（Sora / IBM Plex Sans / JetBrains Mono）均不含 CJK 字形：已加入 `assets/fonts/NotoSansSC-Regular.otf`（SIL OFL，noto-cjk 官方 SubsetOTF/SC 完整版，单字重 8.3MB，OTF/CFF 格式，全量简中字形，未做自制子集化），在 `theme.rs` 挂在 Proportional 与 Monospace 家族链尾做回退；`theme.rs` 的测试 `chinese_glyph_rasterises` 验证字形可用。新增非拉丁语言时同样要确认字形覆盖。`tidy()` 对中文安全（只折叠空白），`tidy(&t!("key"))` 是长消息的标准写法。

### 术语表（翻译时必须遵循）

| 英文 | 中文 | 英文 | 中文 |
|---|---|---|---|
| Install / Update / Remove | 安装 / 更新 / 移除 | Games / Setup 页 | 游戏页 / 安装页 |
| advanced options | 高级选项 | add-on | 附加组件 |
| neural rendering | 神经渲染 | frame generation | 帧生成 |
| multi-frame generation | 多帧生成 | engine | 引擎 |
| setup（方案） | 方案 | Try the next setup | 尝试下一套方案 |
| Go back to the previous setup | 回退到上一套方案 | Diagnose | 诊断 |
| Save report | 保存报告 | bridge | 桥接 |
| stable build / release candidate | 稳定版 / 候选版（RC） | upscaling / super resolution | 超分辨率 |
| quality preset | 画质档位 | poster / card | 封面 / 卡片 |
| pinned | 固定 | headless | 命令行 |
| Rescan | 重新扫描 | store（商店） | 商店 |

保留英文不译：DLSS、DLSS 5、ReShade、Feeder / DLSS5-Feeder、OptiScaler、RenoDX、ShortFuse、Streamline、NGX、LumeniteFX、RTXMFG、dgVoodoo2、REFramework、dlss5-bridge、Game Pass、各商店名、快捷键名。产品名与版本号和中文之间留一个空格（如「安装 DLSS 5 神经渲染」「ReShade 已就绪」）。
