# IMPL — Windows 版本开发记录

基线：`dev/windows@0670f89`。本文只记录本次 Windows 开发中发现的文档偏差、未写明细节、技术坑与后续问题，不重复产品/协议契约。

## 1. 开发中发现文档的错误

### 1.1 ONNX Runtime 产物形态与文档不一致

**问题描述**
文档把 `onnxruntime.dll` 列为 Windows 发布必备产物，但 `ort 2.0.0-rc.12` 在当前 Windows/MSVC 预编译包中静态链接 ONNX Runtime，构建目录里没有可复制的 DLL。按文档验收会把正常的静态构建误判为缺文件。

**解决方法**
- 检查 `ort-sys` 构建输出，确认链接行为是 `cargo:rustc-link-lib=static=onnxruntime`。
- 安装脚本在找不到 DLL 时不再告警失败，改为说明静态链接。
- 文档改为“默认静态链接；只有切换到动态链接构建时才随包分发 `onnxruntime.dll`”。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `BUILD.html` | §2 构建根与命令；§7 发布 | 改 | 删除 Windows 必须有 `onnxruntime.dll` 的表述，补充分发形态取决于 `ort` 链接方式。 |
| `PLATFORM.html` | §6.6 发布产物 | 改 | Windows 产物写“`onnxruntime` 静态链接；动态链接构建时附带 DLL”。 |
| `WINDOWS.html` | §6.1 P0 #5；§7.3 验收门槛 | 改 | 验收重点改为 daemon 能加载模型并完成推理，不要求存在外部 DLL。 |
| `frontends/windows/BUILD.html` | §2 构建；§5 发布 | 改 | 产物表注明静态/动态两种形态，避免安装脚本误报。 |

### 1.2 实现状态与文件树已经过期

**问题描述**
README、PLATFORM 与 `frontends/windows` 子树仍写着 P3 未实现、`frontends/windows` 与 `packaging/windows` 待落地、源码树缺少 `settings.rs` 与 Windows 安装脚本。继续按旧状态阅读会误判构建根和验收范围。

**解决方法**
- 把 P3 状态改为已实现；TSF 明确保留为可选 P4，当前分支不包含。
- 根 CORE 与 Windows CORE 的源码树补齐 `settings.rs`、`packaging/windows` 与安装脚本。
- 根 README、PLATFORM、USER、voimed BUILD 的目标描述同步为实际状态。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `README.html` | 分支范围；§3 项目地图；§4 文档表 | 改 | P3 已实现，TSF 为可选 P4；项目地图与组件构建根同步。 |
| `PLATFORM.html` | 顶部状态；§3.1 目录树；§7 迁移阶段 | 改 | P2/P3 已落地；P4 的源码迁移已完成，TSF 子工程仍可选。 |
| `frontends/windows/README.html` | 状态 | 改 | 写明 P3 已实现、P4 未包含。 |
| `frontends/windows/CORE.html` | 状态；§1 文件树 | 改 | 状态改为已实现，文件树加入 `settings.rs`，并说明 `tsf/` 当前不存在。 |
| `frontends/windows/BUILD.html` | 状态；§3 安装 | 改 | 状态改为已实现，安装命令改为 PowerShell 脚本。 |
| `frontends/windows/USER.html` / `PROVIDER.html` | 状态 | 改 | 从“目标 P3/P4”改为“P3 已实现，P4 可选”。 |
| `CORE.html` | §1 根文件树；§4.7 | 改 | 增加 `packaging/windows`，Windows 前端状态改为 P3 已实现。 |

### 1.3 热键实现被描述成低级键盘钩子

**问题描述**
`frontends/windows/CORE` 把热键实现写成 `RegisterHotKey / 低级键盘钩子`。实际实现没有安装 `WH_KEYBOARD_LL`，而是用 `RegisterHotKey` 捕获按下，再用 `GetAsyncKeyState` 轮询 key-up；安全与验收描述因此与代码不一致。

**解决方法**
- 文件树改为 `RegisterHotKey / GetAsyncKeyState key-up`。
- 明确 PTT 的 key-up 由主循环轮询，不依赖低级钩子；Toggle 用按键状态去重，避免自动重复切换。
- `PROVIDER` 保留 `GetAsyncKeyState` 作为允许的 key-up 备选。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/CORE.html` | §1 文件树；§4.4 hotkey.rs | 改 | 删除“低级键盘钩子”的默认描述，写明 `RegisterHotKey + GetAsyncKeyState`。 |
| `frontends/windows/PROVIDER.html` | §4.1 Win32 / UIA 调用 | 改 | 说明 `WH_KEYBOARD_LL` 不是当前默认路径；`GetAsyncKeyState` 是当前 key-up 方案。 |
| `frontends/windows/USER.html` | §2 CONVENTIONS | 不动 | 行为只要求 PTT 有 key-up、不记录其他按键，具体实现由 PROVIDER/CORE 决定。 |

### 1.4 Windows 传输的阻塞与取消描述与实现不符

**问题描述**
transport 文档写 Windows 使用 overlapped I/O 或 `CancelIoEx` 完成阻塞与取消。实际同步命名管道上挂起的 `ReadFile` 会阻塞同一连接的写，必须改为 `PeekNamedPipe` 轮询读；accept 的阻塞取消则需要 `CancelSynchronousIo`，单独 `CancelIoEx` 不能唤醒同步 `ConnectNamedPipe`。

**解决方法**
- Windows 读路径改为 `PeekNamedPipe` 查询可用字节，再同步 `ReadFile`；共享取消标志由 `shutdown` 设置。
- accept 线程复制自己的线程句柄，`shutdown` 时调用 `CancelSynchronousIo`。
- 增加“双方同时在读时仍可双向写”的回归测试。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-transport/PROVIDER.html` | §2 CONVENTIONS（阻塞与取消） | 改 | 改为“accept 用 CancelSynchronousIo；read 用 PeekNamedPipe 轮询 + 取消标志”。 |
| `crates/voime-transport/CORE.html` | §2 CONVENTIONS | 增 | 增加 Windows 读写约束：挂起读会阻塞同连接写，因此读必须先探测可用字节。 |
| `crates/voime-transport/CORE.html` | §5.2 accept 与关闭 | 改 | 调用栈补上 `CancelSynchronousIo`。 |
| `PLATFORM.html` | §5.2 平台传输实现 | 不动 | 只描述平台差异与安全边界，具体取消调用由 transport 文档拥有。 |

### 1.5 Windows 安装产物写成 `voime-setup.exe`

**问题描述**
Windows BUILD 原文把安装流程写成目标 `voime-setup.exe`，但本仓库没有 Inno Setup / MSI 构建链。实际交付物是 PowerShell 安装器、卸载器与模型安装脚本；继续按旧文档找安装器会找不到产物。

**解决方法**
- 提供 `packaging/windows/install.ps1`、`uninstall.ps1` 与 `scripts/install.ps1`、`scripts/install_models.ps1`。
- 安装脚本完成构建、复制二进制、登录启动、模型安装；卸载脚本清理安装目录、启动项，并支持按需删除设置与模型。
- 文档改为 PowerShell 命令，并写明 `-NoBuild`、`-NoModels`、`-NoStartup`、`-Prefix` 等实际参数。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/BUILD.html` | §3 安装与启动 | 改 | 用 PowerShell 安装/卸载命令替换 `voime-setup.exe`，列出参数与默认目录。 |
| `BUILD.html` | §3.6 Windows 从源码安装；§8 升级 / 卸载 | 改 | 增加脚本路径、默认前缀、登录启动与卸载选项。 |
| `README.html` | §2 用户交互（安装 / 启动） | 不动 | 只描述用户可见结果，具体脚本入口由 BUILD 拥有。 |

## 2. 开发中发现文档没写明的细节

### 2.1 缺少 Fcitx 等价的 Windows 设置窗口契约

**问题描述**
Windows USER 只规定“托盘菜单设置 Mode / Hotkey”，没有定义独立配置窗口。实现时无法确定哪些选项必须出现、如何保存、如何取消，也无法保证和 Fcitx 配置项一一对应。

**解决方法**
- 新增 `settings.rs`：模式下拉框、热键输入框、浮层与登录启动复选框、daemon 启动 / 停止 / 诊断按钮、保存 / 取消。
- 设置窗口在独立线程运行，保存通过通道交回主线程应用，避免与热键注册、浮层和 IPC 状态竞争。
- UI 章节补充窗口尺寸、字体、配色与控件契约。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §4.1 配置；§5 UI | 改 | 增加设置窗口入口、字段、保存语义与视觉契约。 |
| `frontends/windows/CORE.html` | §1 文件树；§4.10 settings.rs；§5.6 | 增 | 增加设置窗口类型、参数与线程/保存调用栈。 |
| `frontends/windows/README.html` | §2 用户交互（配置） | 改 | 写明“托盘右键菜单或设置窗口”。 |
| `README.html` | §2 用户交互（配置） | 改 | Windows 配置入口补上设置窗口。 |

### 2.2 daemon 手动启停与登录启动细节不足

**问题描述**
文档只说“托盘拉起 daemon”，没有写明用户如何手动启动、停止、诊断 daemon，也没有说明安装器使用哪种登录启动机制、停止操作走什么命令。实现时容易把 daemon 做成 Windows 服务或让托盘退出时误杀 daemon。

**解决方法**
- 明确每用户 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 启动 `voime-tray.exe`，不使用 Session 0 服务。
- 托盘自动启动同目录 `voimed.exe`；右键菜单与设置窗口提供“启动 daemon / 停止 daemon / 诊断”。
- 停止操作通过 `voime ctl quit`；退出托盘只关闭前端与浮层，daemon 继续运行，需单独停止。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §4.6 daemon 生命周期 | 增 | 增加登录启动、自动拉起、手动启停、诊断、退出托盘的行为表。 |
| `README.html` | §2 用户交互（停止 daemon） | 改 | 增加 Windows 托盘菜单 / 设置窗口的停止入口。 |
| `frontends/windows/BUILD.html` | §3 安装与启动 | 改 | 写明登录启动注册表项、托盘拉起 daemon、停止命令。 |
| `crates/voimed/BUILD.html` | §1 环境（服务管理） | 改 | 把 Windows 目标改为“登录启动 / 托盘拉起已实现”。 |

### 2.3 配置路径与保存方式未固定

**问题描述**
Windows USER 说“设置载体由实现决定”，而 WINDOWS 附录给出 `%APPDATA%\voime\config.json`。如果不固定路径，doctor、安装器和用户排障无法给出确定位置；保存实现也容易在 Windows 文件替换语义上出错。

**解决方法**
- 固定配置路径为 `%APPDATA%\voime\config.json`。
- 使用临时文件写入后替换；替换前删除已有文件，避免 Windows 上 `rename` 不能覆盖目标。
- 设置窗口保存时先校验热键字符串，再由主线程统一应用并发送 `Configure`。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §4.1 配置 | 改 | 写明配置路径、临时文件写入与立即同步语义。 |
| `frontends/windows/CORE.html` | §4.2 config.rs | 改 | 写明 `save` 的原子替换与 Windows 覆盖行为。 |
| `WINDOWS.html` | §9 附录映射 | 不动 | 路径已在此给出；前端 USER 只引用，不重复定义。 |

### 2.4 浮层缺少可实现的数值契约

**问题描述**
Windows USER §5 明确要求“实现前补充几何、字体、圆角与多显示器定位数值”，但没有给出任何基准值，导致浮层实现和验收无法逐项对照。

**解决方法**
- 以 96 DPI 为基准实现内边距 10、最小宽度 120、最大文本宽度 560、最小高度 20，按目标窗口 DPI 缩放。
- 字体固定为 `DEFAULT_GUI_FONT`，颜色使用 `COLOR_INFOTEXT` / `COLOR_INFOBK`，整体不透明度 235/255，方角。
- 定位优先跟随目标光标下方 4 px，无光标时放在窗口左下外侧，并限制在工作区内。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §5 UI | 改 | 写入上述几何、字体、颜色、不透明度与定位契约。 |
| `WINDOWS.html` | §4.1 / §4.4 | 不动 | 只保留方案对比与风险；浮层数值契约由 windows USER §5 唯一拥有。 |
| `frontends/windows/CORE.html` | §4.6 preview.rs | 不动 | 签名不变；数值属于 USER 的视觉契约。 |

### 2.5 启动顺序与热键注册的窗口依赖未说明

**问题描述**
文档的启动调用栈把 IPC 连接放在 `Tray::create` 和热键注册之前，但 `RegisterHotKey` 需要一个窗口句柄。实现必须先在主线程创建托盘窗口，再注册热键，否则热键无法注册。

**解决方法**
- 启动顺序改为：单实例 → 加载配置 → 创建托盘窗口 → 创建浮层 → 连接 daemon → 注册热键 → 进入消息循环。
- 托盘窗口同时承担通知区回调、`WM_HOTKEY` 和菜单消息循环。
- 文档 5.1 调用栈更新为实际顺序。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/CORE.html` | §5.1 启动与连接 | 改 | 按实际顺序列出 Tray → Overlay → try_connect → sync_hotkey。 |
| `frontends/windows/CORE.html` | §4.1 main.rs | 改 | 写明 `ensure_single_instance` 后可先建窗口/托盘，再注册热键。 |
| `frontends/windows/USER.html` | §2 CONVENTIONS | 不动 | 用户契约只关心“热键可用”，不关心窗口创建顺序。 |

### 2.6 Windows 模型安装选项与缓存目录未说明

**问题描述**
文档只给出模型目标目录，没有说明 Windows 缓存目录、独立模型安装脚本、跳过构建/模型的安装参数。用户排障时容易把缓存、虚拟环境和模型目录混在一起。

**解决方法**
- `tools/install_models.py` 在 Windows 默认使用 `%LOCALAPPDATA%\voime\cache` 与 `%LOCALAPPDATA%\voime\models`。
- `scripts/install_models.ps1` 在缓存目录创建 venv，按 requirements 哈希复用依赖。
- 主安装器支持 `-NoModels`、`-NoBuild`、`-NoStartup`、`-Prefix`，卸载器支持 `-RemoveSettings`、`-RemoveModels`。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `BUILD.html` | §4 安装模型 | 改 | 增加 Windows 命令、缓存目录、venv 与模型目录的关系。 |
| `frontends/windows/BUILD.html` | §3 安装与启动 | 增 | 增加模型安装与跳过模型的参数说明。 |
| `README.html` | §2 用户交互 | 不动 | 只描述安装结果，不列脚本参数。 |

## 3. 其它技术上踩的坑

### 3.1 同步命名管道的挂起读会阻塞同连接写

**问题描述**
托盘启动后显示“未连接”，右键盘菜单也没有响应；daemon 日志显示已收到 `Hello` 并发送 `Snapshot`，但客户端一直收不到，`voime ctl status` 也会挂住。根因是 Windows 同步命名管道上，一个已挂起的 `ReadFile` 会阻塞同一连接上的写；服务端 reader 线程在等客户端输入时，writer 线程无法把 `Snapshot` 写出去，托盘主线程卡在握手阶段，消息循环随之停摆。

**解决方法**
- 读路径改为 `PeekNamedPipe` 轮询可用字节，只有有数据时才调用 `ReadFile`，避免长期挂起的同步读。
- `Shared` 增加取消标志，`shutdown` 设置标志并调用 `CancelIoEx`，轮询循环可退出。
- 增加“双方同时挂起读，仍能双向写”的 transport 回归测试。
- 修复后实测托盘 `addon=Connected`，左键可打开设置窗口，退出可正常回收。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-transport/PROVIDER.html` | §2 阻塞与取消 | 改 | 写明同步命名管道挂起读会阻塞写，因此 Windows 读采用 PeekNamedPipe 轮询。 |
| `crates/voime-transport/CORE.html` | §2 CONVENTIONS | 增 | 增加 Windows 读写约束和取消标志说明。 |
| `crates/voime-transport/USER.html` | §4.2 Listener / Stream | 不动 | 对外契约仍是“read 可阻塞、shutdown 可唤醒”，不暴露轮询细节。 |

### 3.2 `CancelIoEx` 不能唤醒同步 `ConnectNamedPipe`

**问题描述**
listener 关闭时只调用 `CancelIoEx`，但 accept 线程阻塞在同步 `ConnectNamedPipe`，`shutdown` 后线程仍不退出，daemon 关闭会卡住。

**解决方法**
- accept 线程复制当前线程句柄，记录到 listener 状态。
- `shutdown` 对阻塞线程调用 `CancelSynchronousIo`，同时保留 `CancelIoEx` 清理连接 I/O。
- 增加“阻塞 accept 能被 shutdown 唤醒”的回归测试。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-transport/PROVIDER.html` | §2 阻塞与取消 | 改 | 明确同步 `ConnectNamedPipe` 的取消使用 `CancelSynchronousIo`。 |
| `crates/voime-transport/CORE.html` | §5.2 accept 与关闭 | 改 | 调用栈增加 `CancelSynchronousIo`。 |
| `PLATFORM.html` | §5.2 平台传输实现 | 不动 | 只列平台差异，不复制具体 Win32 调用。 |

### 3.3 windows-sys 的 `DuplicateHandle` 与 `HKEY` 类型

**问题描述**
按直觉把 `DuplicateHandle` 放在 `Win32::System::Threading` 会编译失败；它实际在 `Win32::Foundation`。另外 `HKEY` 在当前 `windows-sys` 中是裸指针，不能用 `0` 初始化，需要 `std::ptr::null_mut()`。

**解决方法**
- 修正 `DuplicateHandle` 导入模块。
- `RegOpenKeyExW` 的 `HKEY` 初始化改为 `null_mut()`。
- 通过 MSVC 编译与 clippy 验证导入路径。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-transport/CORE.html` | §4.3 windows.rs | 不动 | 签名不变；导入路径属于实现细节，不进入契约。 |
| `crates/voime/CORE.html` | §4.7 doctor.rs | 不动 | doctor 的公开行为不变；不记录底层 Win32 类型。 |

### 3.4 `windows` crate 控件样式与空句柄

**问题描述**
`CBS_DROPDOWNLIST`、`BS_AUTOCHECKBOX`、`ES_AUTOHSCROLL` 是 `i32` 常量，而 `WS_TABSTOP`、`WS_BORDER` 是 `WINDOW_STYLE` 新类型，不能直接位或。`CreateWindowExW` 的父窗口、菜单、实例参数也不能随意传 `None`，否则泛型无法推导。

**解决方法**
- 组合样式时把 `WINDOW_STYLE.0` 转成 `i32` 再合并。
- 顶层窗口使用 `HWND::default()`、`HMENU::default()`、`HINSTANCE::default()` 表示空句柄。
- 子控件菜单句柄用 `HMENU(id as usize as *mut c_void)` 传递控件 ID。
- 用 `WM_SETFONT` + `DEFAULT_GUI_FONT` 统一系统字体。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/CORE.html` | §4.10 settings.rs | 不动 | 窗口/控件创建与类型转换属于实现细节，不改变设置窗口契约。 |
| `frontends/windows/USER.html` | §5 UI | 不动 | 视觉契约只描述尺寸、字体、控件关系，不描述 Win32 类型。 |

### 3.5 Windows 上 `fs::rename` 不能覆盖已有文件

**问题描述**
配置第一次保存成功，第二次保存时 `fs::rename(temp, config.json)` 在 Windows 上失败，因为目标已存在。用户修改 Mode / Hotkey 会表现为“保存失败”。

**解决方法**
- 先写 `.json.tmp`，若目标存在则先 `remove_file`，再 `rename`。
- 保留错误上下文；失败时设置窗口显示 MessageBox。
- 模型安装器继续使用已有的备份/替换流程。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/CORE.html` | §4.2 config.rs | 改 | 写明临时文件 + 删除旧文件 + rename 的保存流程。 |
| `frontends/windows/USER.html` | §4.1 配置 | 不动 | 只规定持久化结果与立即同步，不规定文件系统实现。 |

### 3.6 `SetWindowTextW` 需要 NUL 结尾

**问题描述**
浮层把 `text.encode_utf16().collect()` 直接传给 `SetWindowTextW`，字符串没有结尾 NUL，Windows 会继续读到缓冲区之外，可能显示乱码或崩溃。

**解决方法**
- 测量文本时使用无 NUL 的 UTF-16 切片。
- 设置窗口文本前重新追加 `0u16`，再传 `PCWSTR`。
- 所有 registry、菜单、消息框字符串统一走带 NUL 的 `to_wide`。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/CORE.html` | §4.6 preview.rs | 不动 | 签名与行为契约不变；NUL 终止是 Win32 实现细节。 |
| `frontends/windows/USER.html` | §5 浮层内容 | 不动 | 用户只看到纯文本，不涉及字符串内存布局。 |

### 3.7 `RegisterHotKey` 没有 key-up，且会受自动重复影响

**问题描述**
PTT 需要松手停止，但 `RegisterHotKey` 只提供按下通知；Toggle 按住键时还可能收到重复通知，造成连续切换。若直接把 `WM_HOTKEY` 当点击，会出现录音停不下来或 Toggle 反复开关。

**解决方法**
- 用 `GetAsyncKeyState` 在主循环轮询 key-up，PTT 释放后发送 `Stop`。
- 用 `hotkey_down` 去重，忽略按住期间的重复 `WM_HOTKEY`；key-up 后复位。
- 进入 finalizing 后忽略新的热键启动，避免和 daemon 的 `Busy` 竞争。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §2 CONVENTIONS；§3.1 后台听写 | 改 | 明确 PTT key-up 和 Toggle 去重语义。 |
| `frontends/windows/PROVIDER.html` | §4.1 Win32 / UIA 调用 | 改 | 写明 `GetAsyncKeyState` 是当前 key-up 轮询方案。 |
| `frontends/windows/CORE.html` | §5.2 热键到提交 | 改 | 调用栈补上 key-up 轮询与重复按键过滤。 |

### 3.8 托盘隐藏窗口无法可靠成为前台

**问题描述**
右键菜单需要 owner 窗口成为前台才能正常显示和消失；最初托盘窗口完全隐藏，`SetForegroundWindow` 可能失败，菜单会不显示或点击外部不消失。

**解决方法**
- 创建 `WS_EX_TOOLWINDOW` 的 1×1 顶层窗口，放在屏幕外并 `SW_SHOWNOACTIVATE`，保留可成为前台的能力。
- 菜单显示前 `SetForegroundWindow`，菜单结束补发 `WM_NULL`。
- 左键托盘直接打开设置窗口，右键打开菜单；退出项和 `WM_CLOSE` 都进入正常关闭路径。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §5 UI（托盘图标） | 改 | 写明左键设置、右键菜单、退出行为与菜单项。 |
| `frontends/windows/CORE.html` | §4.8 tray.rs | 不动 | 窗口实现技巧不改变 Tray 的公开签名，设置窗口契约由 §4.10 拥有。 |
| `frontends/windows/README.html` | §2 用户交互 | 不动 | 只描述用户入口，不复制托盘窗口实现。 |

### 3.9 UIA RuntimeId 与 COM 线程模型

**问题描述**
Windows 没有 Fcitx `InputContext` 等价物，目标绑定依赖 UIA 焦点元素。UIA 返回 `SAFEARRAY`，需要在 COM 初始化后读取；如果直接按 Rust `Vec` 处理或忽略 COM 线程模型，会拿不到 RuntimeId 或崩溃。

**解决方法**
- 主线程以 STA 调用 `CoInitializeEx`，用 `CUIAutomation` 获取焦点元素。
- 读取 `SAFEARRAY` 的 LBound/UBound、访问数据后 `SafeArrayUnaccessData`，最后 `SafeArrayDestroy`。
- UIA 不可用时降级为只绑定前台 HWND；提交前仍复核 HWND 和 RuntimeId。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/PROVIDER.html` | §4.1 Win32 / UIA 调用 | 改 | 补充 COM 初始化、RuntimeId 的 SAFEARRAY 生命周期与 UIA 不可用时的降级。 |
| `frontends/windows/USER.html` | §4.2 目标与提交 | 不动 | 仍规定“HWND + RuntimeId，UIA 不可用时尽力复核”。 |
| `frontends/windows/CORE.html` | §4.5 target.rs | 改 | 补上 `current_runtime_id` 的 SAFEARRAY 读取与销毁说明。 |

### 3.10 SendInput 的 UTF-16、代理对与部分失败

**问题描述**
文本内部是 UTF-8，`SendInput` 需要 UTF-16 单元；emoji 是代理对，按字节或随机边界切分会产生半个字符。长文本一次性发送还可能遇到目标应用处理异常或 UIPI 导致部分事件失败。

**解决方法**
- 统一 `encode_utf16()` 后按 UTF-16 单元注入，每单元发送按下/抬起事件。
- 分块时若块尾是高位代理，则把完整代理对留在同一块。
- 检查 `SendInput` 返回数量；不足时停止后续注入并记录错误，不重试其他窗口。
- 提交前再次调用 `target::focused`，目标变化则放弃。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §4.2 目标与提交 | 改 | 明确代理对不拆分、分批注入、部分失败不重试。 |
| `frontends/windows/CORE.html` | §4.7 commit.rs | 改 | 增加 `to_utf16`、分块与 `send_unicode` 的错误处理签名说明。 |
| `WINDOWS.html` | §4.1 实现要点 | 不动 | 只保留 SendInput 的平台风险与覆盖边界，详细契约在 windows USER/CORE。 |

### 3.11 `ort` 在 Windows GNU 目标没有预编译包

**问题描述**
用 `x86_64-pc-windows-gnu` 做快速检查时，`ort-sys` 直接报“does not provide prebuilt binaries for the target”。这不是代码错误，而是 Windows 推理依赖要求 MSVC 目标；继续在 GNU 下排查会浪费时间。

**解决方法**
- 使用 `x86_64-pc-windows-msvc` + Visual Studio Build Tools 验证 daemon/CLI。
- 前端与 transport 可先用 GNU 做快速反馈，但最终验收以 MSVC 为准。
- 文档明确 Windows 构建使用 MSVC 工具链。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `BUILD.html` | §1 环境（Windows 构建依赖） | 改 | 写明 Windows 只支持 MSVC；GNU 目标没有 ORT 预编译包。 |
| `frontends/windows/BUILD.html` | §1 环境 | 不动 | 前端不依赖 ONNX，GNU/MSVC 均可；只在根文档约束 daemon。 |

### 3.12 XDG/HOME 路径在 Windows 失效

**问题描述**
`voime-core::config::default_model_dir` 和 Python 模型安装器最初只读 `XDG_DATA_HOME` / `HOME`，Windows 上会退到相对目录 `models`，导致 daemon 找不到模型、doctor 报错。

**解决方法**
- Rust 在 Windows 优先使用 `LOCALAPPDATA\voime\models`。
- Python 安装器的默认模型目录与缓存目录在 Windows 使用 `LOCALAPPDATA\voime\...`。
- doctor 输出实际模型目录，安装脚本与 daemon 使用同一默认值。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `BUILD.html` | §4 安装模型（默认目录） | 不动 | 已明确 Windows 使用 `%LOCALAPPDATA%\voime\models`。 |
| `crates/voime-core/USER.html` | §4.1 配置 | 改 | 增加 Windows 默认路径与 XDG 路径的优先级。 |
| `frontends/windows/CORE.html` | §4.2 config.rs | 不动 | 前端配置路径与模型路径分离，模型路径由 core/USER 拥有。 |

## 4. UI 与运行时改进

### 4.1 设置窗口从 Win32 控件迁移到 egui / eframe

**问题描述**
原 `settings.rs` 使用 `STATIC` / `EDIT` / `COMBOBOX` / `BUTTON` 等原生 Win32 控件，字体与配色固定为系统默认，界面与新的托盘/浮层风格不一致，配置项也没有清晰的分组与状态反馈。

**解决方法**
- 设置窗口改用 `eframe` / `egui` 0.32.3（`glow` / OpenGL 后端），不引入 WebView2、Node 或前端构建链。
- 使用系统 UI 字体（Segoe UI / Microsoft YaHei）、跟随系统明暗主题的配色、单一强调色 `#527DFF`、圆角卡片与分段按钮。
- 模式改为三段式选择；热键区域改为录制按钮；浮层/登录启动改为单行复选框；daemon 状态与启停、诊断合并为一行。
- 保存改为 `Save { config, ack }`，由处理线程写盘并把结果回传窗口；注册冲突等错误在窗口内显示，不再弹 MessageBox。
- 设置窗口使用代码生成的品牌图标，避免 eframe 默认图标。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/Cargo.toml` | 依赖 | 改 | 增加 `eframe`、`winit`；保持 `windows` crate 的 Win32 / UIA / GDI 依赖。 |
| `frontends/windows/CORE.html` | §1 文件树；§3 SBOM；§4.10 settings.rs；§5.6 | 改 | 文件树加入 `paths.rs`、`diagnostics.rs`、`control.rs`；SBOM 加入 eframe / egui / winit；设置窗口签名与调用栈同步。 |
| `frontends/windows/USER.html` | §5 UI | 改 | 设置窗口尺寸、主题、控件与热键录制契约。 |
| `frontends/windows/README.html` | Credits | 改 | 补 eframe / egui。 |
| `BUILD.html` / `frontends/windows/BUILD.html` | 运行期依赖 | 改 | 说明设置窗口使用 OpenGL（glow），不依赖 WebView2 / Node。 |

### 4.2 设置窗口改为独立进程，避免 winit 事件循环重建

**问题描述**
winit 只允许每个进程创建一个 `EventLoop`。原实现每次打开设置都新建线程并调用 `eframe::run_native`，第一次正常，关闭后再次打开会报 `winit EventLoopError: EventLoop can't be recreated`。把设置窗口长期挂在托盘进程里又会与托盘消息循环、浮层和退出路径耦合。

**解决方法**
- `voime-tray.exe` 增加 `--settings` 模式：设置窗口在独立进程中运行，每个进程只创建一次事件循环，关闭即退出。
- 托盘打开设置时启动同目录 `voime-tray.exe --settings --status=... [--focus-hotkey]`，并暂停全局热键；设置进程退出后，托盘重新读取 `%APPDATA%\voime\config.json`、应用配置并恢复热键。
- 设置进程内的保存、启动/停止服务由动作线程执行；诊断在窗口内异步执行；保存结果通过 `ack` 回传窗口。
- 设置进程负责在写盘前校验配置；异常通过窗口内提示或 MessageBox 显示，不把错误留给托盘进程。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/CORE.html` | §1 文件树；§4.1 main.rs；§5.6 | 改 | 写明 `--settings` 模式、设置进程、动作线程与托盘重载流程；删除“设置窗口线程”描述。 |
| `frontends/windows/USER.html` | §4.1 配置；§5 UI | 改 | 说明设置保存后由托盘重载，以及设置进程退出后的热键恢复。 |
| `frontends/windows/PROVIDER.html` | §4.1 Win32 / UIA 调用 | 改 | 补充设置进程只创建一个消息循环的约束。 |

### 4.3 浮层重做为圆角主题卡片

**问题描述**
原浮层使用 `COLOR_INFOBK` / `COLOR_INFOTEXT`、`DEFAULT_GUI_FONT`、方角和固定内边距，视觉上仍是系统信息框；高 DPI 下窗口尺寸与字体比例也不一致。

**解决方法**
- 浮层改为圆角卡片：12 px 圆角、12 px 内边距、左侧 3 px 录音强调条、`CS_DROPSHADOW` 阴影。
- 跟随系统明暗主题：浅色背景 `#FAFAFC`、深色背景 `#202124`；文字、边框与强调色分别取两套值。
- 字体改为 `Microsoft YaHei UI`，96 DPI 基准 15 px，随目标窗口 DPI 缩放；最大文本宽度 600。
- 启动时设置 per-monitor v2 DPI awareness，浮层窗口按真实 DPI 计算尺寸与位置。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §5 UI（浮层、主题、几何） | 改 | 更新浮层颜色、字体、圆角、强调条、内边距与最大宽度。 |
| `frontends/windows/CORE.html` | §4.6 preview.rs | 不改签名 | 浮层视觉与 DPI 细节由 USER 拥有；CORE 只保留 `Overlay::show / hide / set_enabled`。 |
| `frontends/windows/PROVIDER.html` | §4.1 Win32 / UIA 调用 | 不动 | GDI 绘制属于实现细节，不改变 provider 边界。 |

### 4.4 启动不显示控制台，并统一进程 DPI 感知

**问题描述**
`voime-tray.exe` 默认是控制台子系统，登录启动或双击时会先出现命令行窗口；浮层、托盘与设置窗口的 DPI 感知不一致，导致高 DPI 下尺寸与位置偏移。

**解决方法**
- `main.rs` 增加 `#![cfg_attr(not(test), windows_subsystem = "windows")]`，release / debug 二进制都以 GUI 子系统启动，测试构建保留控制台输出。
- `main` 在创建任何窗口前调用 `SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)`。
- 启动 daemon / 停止 daemon / 打开设置进程继续使用 `CREATE_NO_WINDOW`，不产生额外控制台。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/CORE.html` | §4.1 main.rs | 改 | 写明 GUI 子系统与 per-monitor v2 DPI awareness。 |
| `frontends/windows/USER.html` | §5 UI（托盘） | 改 | 写明启动不弹出控制台窗口。 |
| `frontends/windows/BUILD.html` | §1 环境；§2 构建 | 改 | 说明设置窗口使用 OpenGL、二进制为 GUI 子系统，不需要额外控制台。 |

### 4.5 配置与 daemon 动作解耦

**问题描述**
设置窗口原先通过内存通道把保存、daemon 启停交回托盘主线程，设置窗口与托盘生命周期强绑定；设置窗口退出后，托盘无法感知配置是否已更新。

**解决方法**
- 设置进程保存配置后退出；托盘在 `tick` 中轮询设置子进程，检测到退出后重新加载配置文件并应用。
- 启动 / 停止服务、诊断在设置进程或托盘各自执行，仍使用 `paths` 定位同目录的 `voimed.exe` / `voime.exe`。
- 新增 `control.rs` 统一封装启动 / 停止服务，避免托盘和设置进程重复拼装命令。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/CORE.html` | §1 文件树；§4.10 settings.rs；§5.6 | 改 | 增加 `paths.rs`、`diagnostics.rs`、`control.rs` 与设置子进程重载流程。 |
| `frontends/windows/USER.html` | §4.6 daemon 生命周期 | 改 | 说明设置窗口保存后由托盘自动重载，不需要重启托盘。 |
| `README.html` / `frontends/windows/README.html` | 用户交互（配置） | 改 | 保持“托盘菜单或设置窗口配置”，不承诺设置窗口与托盘同进程。 |

### 4.6 Toggle 会话保持预览浮层

**问题描述**
Toggle 会话在 daemon 中跨多个 VAD 语音段；原 Windows 前端在每段 Final 后直接 `Overlay::hide()` 并清空目标，导致 Toggle 开启期间预览窗口消失。文档也写成“Windows 浮层在 Final 后隐藏”，与 Toggle 会话语义不一致。

**解决方法**
- 前端新增 `toggle_active`，Toggle 首次按下时立即显示空白浮层；每个 Final 提交后清空文本但调用 `Overlay::show_empty()` 保持浮层，直到 toggle off、目标失焦或断线。
- Toggle 续段在 `SegmentStarted` 时先检查原目标是否仍聚焦；失焦则发送 Stop 并结束会话，不在新窗口重建预览。
- PTT / Continuous 仍在 Final 后隐藏；Toggle 的 ASR 错误（Inference）保留空白浮层，Busy / NoInputContext / 断线结束会话。
- 更新 `overlay.puml` 并重新导出 `overlay.svg`：Final 分支区分 Toggle `show_empty()` 与其他模式 `hide()`。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §2 CONVENTIONS；§3.4；§4.3 | 改 | 明确 Toggle 会话在 Final / 空段 / ASR 错误后保留空白浮层，直到 toggle off、目标失焦或断线；Final 消息行为按模式区分。 |
| `frontends/windows/README.html` | §2 用户交互 | 改 | Toggle 行与临时浮层行说明会话期间保持预览。 |
| `README.html` | §2 用户交互（Toggle、临时文本窗口） | 改 | 移除“Windows 浮层在 Final 后隐藏”的平台差异，统一为 Toggle 保留空白窗口。 |
| `USER.html` | §3.2；§4.4 | 改 | 补充 Toggle 会话预览生命周期与目标失效停止约束。 |
| `frontends/windows/CORE.html` | §4.6 preview.rs；§5.2；§5.3 | 改 | 增加 `Overlay::show_empty`；Final 与失焦调用栈按 Toggle / 其他模式分支。 |
| `frontends/windows/diagram/overlay.puml` / `overlay.svg` | — | 改 | 更新 Final 分支，重新导出 SVG。 |

### 4.7 Windows 托盘与 daemon 改为同生命周期

**问题描述**
此前 `voime-tray` 与 `voimed` 完全解耦：托盘按需拉起 daemon，退出托盘不停止 daemon，而 daemon 也不是 Windows 服务。实际安装只在登录时启动托盘，daemon 独立存活既没有服务管理上的收益，又会在托盘退出或崩溃后留下孤儿进程，继续占用命名管道和录音设备。

**解决方法**
- Windows 前端把 `voimed` 作为自己的子进程启动，并加入设置了 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 的 Job Object；托盘正常退出、崩溃或被强制结束时，系统都会终止 daemon。
- 托盘启动时先以 Control 角色发送 `Quit` 停掉已有的、非本托盘托管的 daemon，等待命名管道释放后再启动自己的子进程，避免多个所有者并存。
- 托盘主循环轮询子进程；daemon 意外退出后清除句柄，并按原有重连/重启节奏重新拉起，托盘保留为唯一监督者。
- 退出路径统一为：关闭 IPC / 热键 / 浮层 → 发送 `Quit` → 等待子进程退出（最多 3 秒）→ 关闭 Job Object 兜底终止。
- 设置窗口不再自行启动或停止 daemon；按钮通过新的 `WM_APP` 托盘命令转交托盘处理，避免设置进程创建出生命周期外的 daemon。
- 托盘菜单和设置窗口的停止项改为“停止服务并退出”，停止服务即退出托盘；设置窗口点击后自行关闭。
- 只修改 `frontends/windows`：`control.rs` 负责子进程、Job Object 与控制通道，`main.rs`、`tray.rs`、`settings.rs` 调整生命周期和命令路由；`crates/voimed`、IPC 与 transport 不变，Linux / Fcitx 行为不受影响。

**影响与边界**
- 手动启动的 `voimed` 会在托盘启动时被停止并替换为托盘子进程；托盘退出时同样会停止它。
- 托盘未运行时，设置窗口的“启动服务”不再生效，只提示先启动托盘。
- Job Object 创建或分配失败时降级为“正常退出时停止 daemon”，崩溃后的清理能力随之失效。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/USER.html` | §4.6 daemon 生命周期 | 改 | 改为服务随托盘启动和退出；停止服务即退出托盘；设置窗口启停转交托盘。 |
| `frontends/windows/README.html` | §2 用户交互（daemon 生命周期） | 改 | 写明托盘是 daemon 的唯一所有者，退出托盘会停止 daemon。 |
| `frontends/windows/CORE.html` | §1 文件树；§4.9 control.rs；§5.1 启动与连接 | 改 | 增加 Job Object、子进程托管、退出顺序与设置命令消息。 |
| `README.html` | §2 用户交互（停止 daemon） | 改 | Windows 不再需要单独停止 daemon，退出托盘即可。 |
| `crates/voimed/BUILD.html` | §1 环境（服务管理） | 改 | 说明 Windows 下 daemon 是托盘子进程，不使用独立服务或独立退出流程。 |
| `WINDOWS.html` | §10 生命周期对比 | 不动 | 方案对比不变；具体 Windows 实现由前端文档拥有。 |
| `PLATFORM.html` | §5.2 平台传输实现；§6 迁移 | 不动 | 平台差异不变；生命周期实现只在 Windows 前端。 |

### 4.8 安装脚本处理文件占用

**问题描述**
`install.ps1` 用 `Stop-Process -Force` 后立即复制 `voimed.exe`；`Stop-Process` 返回时进程镜像句柄尚未完全释放，首次安装稳定报 `文件正由另一进程使用`，第二次等句柄释放后成功。托盘和 daemon 的停止顺序也可能让托盘短暂拉起 daemon。

**解决方法**
- 先停 `voime-tray`，再停 `voimed` / `voime`；等待最多 10 秒确认进程全部退出。
- 复制文件遇到 `IOException` 时重试 20 次（每次 250ms）；卸载删除目录也使用等待与重试。
- 构建前和复制前各执行一次停止流程，避免构建产物或安装目录被占用。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `packaging/windows/install.ps1` | 构建前、复制前 | 改 | 增加进程等待与复制重试。 |
| `packaging/windows/uninstall.ps1` | 卸载 | 改 | 增加进程等待与目录删除重试。 |

## 5. 死代码清理、文档同步与运行日志

### 5.1 无调用面 API 清理

**问题描述**
多轮实现迭代积累了从未被调用的 API 与重复机制：`FireRedVad` 的 `min_speech_ms` / `min_silence_ms` 配置与访问器（真实值由 `segmenter.rs` 常量持有）、`FireRedCtc::new` / `FireRedPunc::new`（管线只用 `*_with_factory`）、`IpcServer` 的 `try_event` / `client_count` / `endpoint`、`target::same`（仅自身测试引用）、`Frontend` 的 `let _ = frontend` 参数等。

**解决方法**
- 删除上述 API 及其专属 helper 与结构体字段；`IpcServer` 移除只被 `endpoint()` 读取的 `endpoint` 字段。
- 配置保存去掉 `exists()+remove_file` 前置删除：`std::fs::rename` 在 Windows 使用 `MOVEFILE_REPLACE_EXISTING`，直接覆盖，替换回归单步原子操作。
- `Config::save` 收敛为 `save(&self)`；删除前端 `ipc::dispatch` 透传包装，改为直接调用 `frontend.on_message`。
- 热键的 `named()` 解析表与 `key_name()` 显示表合并为单一 `NAMED_KEYS` 常量表（键码、显示名、解析别名），新增全表键码唯一性与解析/显示往返的单元测试。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-core/CORE.html` | §4.8 vad/firered.rs；§4.12；§4.15 | 改 | 删除 `min_speech_ms` / `min_silence_ms` 与两个 `new` 构造器及 `find_*_from_session`。 |
| `crates/voimed/CORE.html` | §4.6 ipc_server.rs | 改 | 删除 `try_event` / `client_count` / `endpoint` 与 `endpoint` 字段。 |
| `frontends/windows/CORE.html` | §4.2；§4.3；§4.4 | 改 | `save(&self)` 与 rename 覆盖说明；删除 `dispatch`；补 `NAMED_KEYS` 单表说明。 |

### 5.2 运行日志 `voime-log`

**问题描述**
daemon 与托盘都以无控制台方式运行，`eprintln!` 不可见；出现“浮层滞留、会话不结束、启动即退出”一类状态异常时没有现场可查。需要一种可常驻、默认零开销的取证手段。

**解决方法**
- 新增 `crates/voime-log`：零第三方依赖。`VOIME_LOG` 未设置时完全静默；`1|true|on` 写默认目录（Windows `%LOCALAPPDATA%\voime\logs`，Unix `$XDG_STATE_HOME/voime/logs`）；其他非空值为目录路径。
- 每组件一个追加文件（`voimed.log` / `voime-tray.log` / `voime-settings.log`），行格式 `UTC时间戳 [组件] 消息`；超过 10 MiB 启动时轮转为 `.log.1`。
- daemon 记录状态机迁移、IPC 生命周期、`pending_stop` 锁存/命中、段事件与音频启停；托盘记录消息分发、热键分支、浮层显隐、设置子进程与 daemon 子进程生命周期。识别文本只记长度、不记内容。
- `voime doctor` 增加 `log:` 检查项，报告启用状态与位置。
- 宏 `vlog!` 未启用时不做任何格式化；初始化与写入路径有单元测试（日期换算、目录解析、轮转、行格式）。

**修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-log/*` | 全部 | 增 | README / USER / CORE / BUILD 四篇，环境变量契约与实现归 voime-log 拥有。 |
| `CORE.html` | §1 根文件树；§2 依赖方向；§4.8 | 改 | 增加 `voime-log` crate 与签名摘要。 |
| `PLATFORM.html` | §2；§3.1；§3.2；§6.1；§6.2；§6.3 | 改 | crate 清单由五个变六个；构建矩阵 / cfg 规则 / workspace 片段同步。 |
| `BUILD.html` | §2 构建根与命令 | 改 | 测试命令与构建根说明加入 `voime-log`。 |
| `crates/voimed/*` | USER §4.1；CORE §2 / §3 / §5.1；BUILD §3 | 改 | `VOIME_LOG` 选项、SBOM、启动调用栈、运行表格。 |
| `frontends/windows/*` | USER §4.4；CORE §3 / §5.1；BUILD §3 | 改 | 诊断表日志行、SBOM、启动调用栈（同时修正已删除的 `ipc::dispatch`）、安装表日志行。 |
| `crates/voime/USER.html` | §4.4 doctor 检查 | 改 | 增加 `log:` 检查项与示例输出。 |
