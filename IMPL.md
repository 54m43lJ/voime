# IMPL — 开发实施与文档修订清单

本清单记录开发过程中发现的文档错误、文档未写明的细节与其它技术坑。每个主题只在一个大块中出现；修订建议里的“不动”用于避免在多个文档之间重复定义或互相覆盖。

## 1. 开发中发现文档的错误

### E1. TUI 的 Space 键在两个文档里语义不同

1. **问题描述**

`crates/voime/README.html` §2 把 TUI 的操作概括为 Start / Stop，而 `crates/voime/USER.html` §2、§4.2 与 `crates/voime/CORE.html` §5.2 都规定：PTT / Toggle 模式下 Space 发送 `Toggle`，只有 Continuous 模式才按录音状态发送 Start / Stop。按 README 实现会让 PTT / Toggle 的 TUI 调试路径与 daemon 会话状态机不一致。

2. **解决方法**

- TUI 的 Space 在 `ptt` / `toggle` 下统一发送 `Toggle`，在 `continuous` 下 recording 时发送 `Stop`、否则发送 `Start`。
- TUI 的 Finalizing 状态不拦截 Toggle；daemon 允许 Toggle 在 Finalizing 关闭会话。
- 补充 TUI 手工验证：PTT / Toggle 下 Space 开启会话，再按一次关闭；Continuous 下分别 Stop / Start。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime/README.html` | §2 | 改 | 删除“`voime ctl start` / `voime ctl stop` 式 TUI 操作”的概括，改为“PTT / Toggle 下 Space 发送 Toggle，Continuous 下按录音状态 Start / Stop”。 |
| `crates/voime/USER.html` | §2、§4.2 | 不动 | 已定义 TUI 按键语义；README 链接到本处即可。 |
| `crates/voime/CORE.html` | §5.2 | 不动 | 调用栈已按 Toggle / Start / Stop 分支描述。 |

### E2. `voimed` 启动顺序在 CORE 的两个章节中互相矛盾

1. **问题描述**

`crates/voimed/CORE.html` §4.2 写初始化顺序为“配置 → 核心 → worker → pipeline → 音频 → IPC → daemon”，而 §5.1 的冷启动调用栈是“配置 → pipeline → 判断 worker → 音频 → IPC → daemon”。两处顺序不同，开发者无法判断 GPU worker 与 pipeline 谁先创建，也无法确定失败时先清理谁。

2. **解决方法**

- 以 §5.1 为准：先完成核心配置与 pipeline，再启动选中的 GPU worker，最后打开音频并绑定 IPC。
- 初始化失败按逆序释放已创建资源；worker 增加 `Drop`，音频或 IPC 绑定失败时不会留下孤儿 worker。
- 启动日志记录 model root、backend、endpoint 与 worker ready 状态。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voimed/CORE.html` | §4.2 | 改 | 将初始化顺序改为与 §5.1 一致：配置 → core → pipeline → worker → audio → IPC → daemon。 |
| `crates/voimed/CORE.html` | §5.1 | 不动 | 已是实际顺序，作为启动顺序的权威调用栈。 |
| `CORE.html` | §5.1 | 不动 | 根 CORE 的冷启动调用栈与 §5.1 一致。 |

### E3. `voimed` 退出顺序在 README 与 CORE 中不一致

1. **问题描述**

`crates/voimed/README.html` §1.2 写 daemon 退出时先停 worker，再停止 pipeline、音频与 IPC；`crates/voimed/CORE.html` §5.7 的顺序是 worker → 广播 `Bye` → IPC shutdown → pipeline shutdown → audio stop。两处都涉及“先停 worker”，但 `Bye`、IPC、pipeline、音频的相对顺序不同，退出时可能出现客户端收不到 `Bye` 或音频流先于 pipeline 停止。

2. **解决方法**

- 采用 CORE §5.7：先 `Worker::shutdown`，再广播 `Bye`，给 writer 线程短暂 flush 时间，然后关闭 IPC、pipeline 与音频。
- pipeline shutdown 失败不能让音频 `stop` 被跳过；音频停止放在无条件执行的收尾路径。
- 退出路径统一走 `Daemon::request_shutdown`，避免各分支自行置位。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voimed/README.html` | §1.2 | 改 | 将退出顺序改为链接 `crates/voimed/CORE.html` §5.7，不再用一句“先停 worker，再停 pipeline / 音频 / IPC”概括。 |
| `crates/voimed/CORE.html` | §5.7 | 不动 | 已定义完整退出顺序；README 引用本处。 |

### E4. FireRed 的 EOF 语义与 GPU 协议总则冲突

1. **问题描述**

`crates/voime-gpu-common/USER.html` §4.7 规定：daemon 未发送 `Shutdown` 就关闭 stdin / stdout 时，worker 视为异常关闭并以非零退出。但 `crates/voime-gpu-firered/CORE.html` §4.6 写“EOF 按 Shutdown 处理”，会让 FireRed worker 在管道异常断开时发送 `Bye` 并以成功退出，daemon 无法区分正常关闭和崩溃。

2. **解决方法**

- FireRed 读帧线程在干净 EOF 时发送 `Abort` 命令，不发送 `Bye`，`run` 返回 `RunExit::Abort` 后以非零退出。
- 只有收到完整 `Request::Shutdown` 才发 `Bye` 并以 0 退出。
- Qwen worker 保持同一语义：EOF 与帧级错误均走 `Abort`。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-gpu-firered/CORE.html` | §4.6、§5.5 | 改 | 删除“EOF 按 Shutdown 处理”，改为“EOF 不产生 `Bye`，释放资源后非零退出”。 |
| `crates/voime-gpu-common/USER.html` | §4.7 | 不动 | 已定义 EOF 为异常关闭；FireRed CORE 对齐本处。 |
| `crates/voime-gpu-qwen3/CORE.html` | §5.6 | 不动 | Qwen 的关闭 / 异常退出语义已与 common 一致。 |

### E5. `reset_after_segment` 在软切分与 `FinishSegment` 两处含义冲突

1. **问题描述**

`crates/voime-core/CORE.html` §2 说 `reset_after_segment` 保持 `collecting=true` 并重新打开 Preview 门禁；§5.3 的 note 又说 `FinishSegment` 的 VAD / 分段器由 `reset_after_segment` 重置。软切分要“继续采集下一段”，硬切分要“停止采集”，同一个函数无法同时满足两种语义。

2. **解决方法**

- 拆成两个内部路径：软切分使用 `reset_after_segment`（保持 listening / collecting、重开 Preview、发送下一段 `Recording(true)`）。
- 硬切分使用 `reset_after_finish`（清空 listening / collecting、关闭 Preview、清空完成段队列），之后只接受新的 `StartSegment`。
- `FinishSegment` 先排空音频、取消 Preview、`force_end`、交付 `SegmentReady` / `SegmentDropped`，再发送 `Recording(false)` 并走硬重置。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-core/CORE.html` | §5.3 | 改 | 把 `FinishSegment` 的“reset_after_segment”改为“reset_after_finish（硬重置）”，并说明软切分仍使用 `reset_after_segment`。 |
| `crates/voime-core/CORE.html` | §2、§5.2 | 不动 | 已定义软切分继续采集与 `Recording(true)` 续段；硬切分引用 §5.3。 |

### E6. 模型清单示例把 `size` 写成 0，但契约要求校验字节大小

1. **问题描述**

根 `CORE.html` §4.8 的 schema 5 示例使用 `"size": 0`，同节字段表却要求逐文件校验“大小与摘要”；按示例生成的 manifest 会让实现要么把 0 当通配符，要么拒绝所有示例文件。安装脚本与运行期校验无法对“0 是占位还是真实大小”达成一致。

2. **解决方法**

- manifest 的 `files[].size` 必须是真实字节数；`verify` 严格比较实际大小，0 不视为通配符。
- 安装脚本计算每个文件的实际大小与 SHA-256 后再写 manifest；示例改成非零真实值。
- 测试同时覆盖大小不符与 SHA 不符，避免只校验摘要。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `CORE.html` | §4.8 | 改 | 将示例中的 `"size": 0` 改为真实字节数，并在字段表明确“0 不是通配符”。 |
| `crates/voime-core/USER.html` | §4.3 | 不动 | 已要求文件存在、非空、大小与 SHA-256 校验；根 CORE 示例对齐即可。 |

### E7. `voime-core` 文件树中的 `pub mod final;` 不是合法 Rust

1. **问题描述**

`crates/voime-core/CORE.html` §1 写 `pub mod final;`，但 `final` 是 Rust 关键字，直接声明无法编译。实现使用 `pub mod r#final;` 并在 `lib.rs` 重导出 `Final`，而根 `CORE.html` §5.2 又直接使用 `voime_core::Final`，文档与代码的模块可见性不一致。

2. **解决方法**

- `lib.rs` 使用 `pub mod r#final;`，并 `pub use r#final::Final;` 保持根 CORE 的 `voime_core::Final` 调用名。
- 其它模块继续通过 `crate::final::Final` 或重导出访问；不新增第二套类型。
- 在核心 CORE 的文件树里注明 `r#final` 的转义写法。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-core/CORE.html` | §1、§4.14 | 改 | 将 `pub mod final;` 改为 `pub mod r#final;`，并注明 `Final` 通过 `lib.rs` 重导出。 |
| `CORE.html` | §4.1、§5.2 | 不动 | 继续使用 `voime_core::Final`；核心 CORE 负责解释模块转义。 |

## 2. 开发中发现文档没写明的细节

### M1. IPC 无字段消息的线格式没有唯一写法

1. **问题描述**

`crates/voime-ipc/USER.html` §2 说“消息类型是 JSON 对象的唯一键”，§4.2 / §4.3 却用 Rust unit variant 列出 `ClearHistory`、`Status`、`Quit`、`HistoryCleared`、`Bye`；按 serde 默认会编码成 JSON 字符串，按“对象唯一键”又会编码成 `{"Name":{}}`。Rust 客户端与 C++ 前端对同一条消息可能生成两种线格式。

2. **解决方法**

- Rust 枚举使用空结构体变体 `ClearHistory {}`、`Status {}`、`Quit {}`、`HistoryCleared {}`、`Bye {}`，统一编码为 `{"Name":{}}`。
- C++ 解析器同时接受 JSON 字符串和对象两种形式，兼容旧客户端。
- 协议测试固定无字段消息的文本，避免后续回退。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-ipc/USER.html` | §2、§4.2、§4.3 | 改 | 明确无字段消息写作 `{"Name":{}}`，并在枚举定义中使用空结构体变体。 |
| `crates/voime-ipc/USER.html` | §4.6 | 增 | 增加 `ClearHistory` / `Bye` 的对象形式 JSON 示例。 |
| `crates/voime-ipc/CORE.html` | §2 | 不动 | 序列化策略由 USER 定义；CORE 只说明使用外部标签。 |
| `frontends/fcitx/PROVIDER.html` | §4.5 | 不动 | 解析兼容窗口由 IPC USER 统一描述，不在前端重复线格式。 |

### M2. 可选 `cmvn.ark` 的文件格式没有定义

1. **问题描述**

`crates/voime-core/CORE.html` §4.8 与 `PROVIDER.html` §4.2 只说 VAD 的 CMVN 来自可选的 `cmvn.ark`，没有说明它是 Kaldi 二进制 `FM` / `DM` 矩阵、`F` / `D` 向量还是文本，也没有说明统计量是“count + sums + sumsquares”还是“mean + inverse stddev”。FireRed 的 `cmvn.ark` 也存在同样问题，直接按文本或单一行数解析会读错统计量。

2. **解决方法**

- 实现同时接受 Kaldi 二进制矩阵 / 向量（`FM`、`DM`、`F`、`D`）与文本数值。
- 统计量按“count + sums + squares”计算 mean / inverse stddev；对已预计算的两行统计保留兼容路径。
- 维度必须与 fbank bin 数一致；缺失或解析失败按模型加载失败处理。
- 增加 parser 单测，覆盖二进制 f32、二进制 f64 与文本三种输入。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-core/PROVIDER.html` | §4.2 | 改 | 明确 `cmvn.ark` 的 Kaldi ark 格式、统计量布局、维度与失败行为。 |
| `crates/voime-core/CORE.html` | §4.8 | 不动 | 只说明文件来源与校验；格式细节归 PROVIDER。 |
| `crates/voime-gpu-firered/PROVIDER.html` | §4.1.3 | 改 | 增加同样的 `cmvn.ark` 格式说明；与核心共用同一 Kaldi 约定。 |
| `crates/voime-gpu-firered/CORE.html` | §4.10 | 不动 | 已说明读取统计并计算 mean / inverse stddev；格式引用 PROVIDER。 |

### M3. daemon 传给 Qwen worker 的 `model_dir` 是模型根还是模型子目录

1. **问题描述**

`crates/voime-gpu-common/USER.html` §1 只规定 `model_dir` 是“模型目录绝对路径”，布局由 worker 定义；`crates/voime-gpu-qwen3/USER.html` §4.1 把模型放在 `models/qwen3-asr-1.7b/`，但 `voimed` 的 Worker 章节没有写明 Hello 里传根目录还是子目录。Qwen worker 的 `Manifest::load(model_dir)` 需要子目录，否则找不到 `manifest.json`。

2. **解决方法**

- daemon 对 FireRed 传模型根目录（内部再找 `fireredasr2-aed/` 与 `fireredpunc/`）。
- daemon 对 Qwen3 传 `model_root/qwen3-asr-1.7b`，因为 Qwen worker 的模型目录本身就是 manifest 目录。
- worker 仍只接受绝对路径；daemon 在启动前把 model root 规范化为绝对路径。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voimed/CORE.html` | §4.6 | 增 | 写明 `Worker::spawn` 按 backend 计算 Hello 的 `model_dir`：FireRed 传根目录，Qwen3 传 `qwen3-asr-1.7b` 子目录。 |
| `crates/voime-gpu-qwen3/USER.html` | §4.1 | 增 | 明确 Hello `model_dir` 指向包含 manifest 的 `qwen3-asr-1.7b/` 目录。 |
| `crates/voime-gpu-common/USER.html` | §4.2 | 不动 | `model_dir` 的布局归 worker 定义；本处只保留绝对路径与透传契约。 |

### M4. worker 的 runtime / device / 日志参数由谁传递没有写全

1. **问题描述**

FireRed 文档说 EP 在编译时固定，daemon 文档说 worker 由 `voimed` 启动，但没有写清 daemon 是否传 `--runtime`、`--log-level`，以及 `VOIME_GPU_DEVICE` / `VOIME_FIRERED_RUNTIME` 的优先级。实现时容易出现 daemon 用环境变量、worker 用 Hello 字段、安装脚本又用第三个来源的三套来源。

2. **解决方法**

- EP 以 worker 编译 feature 为准；daemon 只在显式设置 `VOIME_FIRERED_RUNTIME` 时传递 `--runtime`，worker 发现与编译 feature 不一致就拒绝。
- 设备只通过 Hello 的 `device` 字段传递；daemon 可用 `VOIME_GPU_DEVICE` 提供默认值，空串表示 backend 默认设备。
- `--log-level` 只影响 worker stderr 日志级别，不改变协议 stdout；模型目录只从 Hello 取，不从环境变量猜。
- 启动失败只标记 backend 不可用，不阻塞 CPU Final。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voimed/CORE.html` | §4.6 | 增 | 列出 `Worker::spawn` 的 runtime / device / log level 来源与优先级。 |
| `crates/voime-gpu-firered/CORE.html` | §4.3 | 增 | 写明 `--runtime`、`VOIME_FIRERED_RUNTIME` 与编译 feature 的关系；模型路径只来自 Hello。 |
| `crates/voime-gpu-qwen3/CORE.html` | §4.2 | 增 | 写明 Qwen 只使用编译 feature，`device` 来自 Hello，`log_level` 只写 stderr。 |
| `crates/voime-gpu-common/USER.html` | §4.2 | 不动 | Hello 字段语义已定义；进程参数不属于帧协议。 |

### M5. doctor 读取的 worker 日志行格式没有定义

1. **问题描述**

`crates/voime/USER.html` §4.4 说 doctor 从 daemon 日志读取 worker ready / running；`crates/voime/CORE.html` §4.8 也把日志作为状态来源，但没有规定日志行的稳定格式。实现只能靠字符串搜索，日志文案一改 doctor 就失明。

2. **解决方法**

- daemon 固定写 `gpu worker <backend> ready`、`gpu worker <backend> running`、`gpu worker <backend> failed` / `unavailable`。
- doctor 只匹配 backend 名与这些状态词，不解析识别文本。
- `VOIME_LOG` 自定义目录时，doctor 用同一目录读取 `voimed.log`。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voimed/CORE.html` | §2 | 增 | 定义 worker 状态日志的稳定行格式（ready / running / failed / unavailable）。 |
| `crates/voime/CORE.html` | §4.8、§5.5 | 改 | 把“读取日志中的 ready / running”改为引用固定行格式，避免文案耦合。 |
| `crates/voime/USER.html` | §4.4 | 不动 | 已说明状态来源；格式归 voimed CORE。 |

### M6. GPU 安装的离线包 / 预编译 worker 目录布局没有定义

1. **问题描述**

GPU 文档要求安装脚本部署“预编译 worker / runtime 包”，也列出 `--source-dir`，但没有说明 source dir 内的目录结构、worker 二进制放在哪里、runtime 与模型如何分目录。实现只能自行约定，导致 `--source-dir` 在不同脚本里含义不同。

2. **解决方法**

- 约定 `--source-dir/bin/voime-gpu-firered` 与 `--source-dir/bin/voime-gpu-qwen3` 为可选预编译 worker；存在时直接安装，否则在对应 crate 内按 feature 构建。
- 模型仍按 `--source-dir/<model-id>/` 复制；runtime 目录由安装包自行放置，不由 worker 从全局库路径查找。
- Windows 使用同名 `.exe`。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-gpu-firered/USER.html` | §4.4 | 增 | 写明 `--source-dir/bin/` 预编译 worker 布局与缺失时的构建回退。 |
| `crates/voime-gpu-qwen3/USER.html` | §4.4 | 增 | 同上，并区分 Vulkan / CUDA 预编译包。 |
| `README.html` | §2 | 不动 | 根 README 只保留安装入口；布局细节归 GPU 组件 USER。 |

### M7. Windows 核心安装如何获得模型没有写明

1. **问题描述**

根 README 与 Windows 文档说 `scripts/install-core.ps1` 安装核心模型，但没有说明它从网络下载、从仓库 `models/` 复制，还是要求用户先准备离线目录。实现如果没有 Python 或 `-SourceDir`，无法凭空得到两个核心模型。

2. **解决方法**

- `install-core.ps1` 在未指定 `-SourceDir` 时调用 `scripts/install-core-models.py`，由它下载 / 缓存并生成 schema 5 manifest。
- 指定 `-SourceDir` 时同样通过 Python 安装器做校验与 manifest 生成，不直接复制未校验目录。
- 没有 Python 且没有 `-SourceDir` 时明确失败，提示传 `-SourceDir` 或 `-NoModels`。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/README.html` | §2 | 增 | 写明核心模型的两种来源：Python 安装器下载 / 缓存，或 `-SourceDir` 离线源；运行期不需要 Python。 |
| `frontends/windows/PROVIDER.html` | §4.5 | 增 | 在安装权限与路径表中补充 Python 模型安装器的调用关系。 |
| `README.html` | §2 | 不动 | 根 README 只列安装入口；模型来源细节归 Windows 文档。 |

### M8. Fcitx 系统 addon 的 ABI 诊断具体检查什么没有写明

1. **问题描述**

`frontends/fcitx/CORE.html` §4.6 与 `PROVIDER.html` §4.3 要求 doctor 检查系统 addon 路径与 ABI，但没有说明 ABI 是编译期版本、`pkg-config` 版本、动态库依赖还是 addon 描述文件。实现如果只检查文件存在，会把 ABI 不匹配的库误报为正常。

2. **解决方法**

- 系统 addon 路径从 `pkg-config --variable=libdir Fcitx5Core` 解析，再拼 `/fcitx5`。
- 描述文件固定检查 `/usr/share/fcitx5/addon/voime.conf`（以及 `/usr/local` 回退）。
- ABI 检查 `pkg-config --atleast-version=5.1.23 Fcitx5Core` 并用 `ldd` 检查动态依赖；不检查 `inputmethod` 条目与 `FCITX_ADDON_DIRS`。
- `voime doctor` 与 addon 自身的 diagnostics 使用同一规则。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/fcitx/CORE.html` | §4.6 | 增 | 写明 ABI 检查由 `pkg-config` 最低版本 + `ldd` 依赖解析组成。 |
| `frontends/fcitx/PROVIDER.html` | §4.3 | 增 | 在系统路径表中补充 libdir 解析与 ABI 判定规则。 |
| `crates/voime/CORE.html` | §4.8 | 不动 | doctor 的检查项已定义；具体 ABI 方法归 fcitx 文档。 |

### M9. `voime ctl backend` 与全局 `--endpoint` 的 CLI 行为没有写清

1. **问题描述**

`crates/voime/USER.html` §4.1 规定 `ctl backend` 不接受 `--endpoint`，但 CLI 把 `--endpoint` 定义为全局参数，clap 会在任意子命令下解析成功。文档没有说明“解析后拒绝”还是“从语法上不提供”，实现容易把它静默忽略。

2. **解决方法**

- 保留全局 `--endpoint` 以复用其它子命令，但在 `ctl backend` 动作入口显式检查；出现该参数时打印错误并以退出码 2 结束。
- `ctl backend` 只写 `[final].backend`，不连接 daemon、不读取端点。
- 帮助文本继续说明该动作不需要端点。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime/USER.html` | §4.1 | 改 | 明确 `ctl backend` 即使全局解析到 `--endpoint` 也会拒绝并以退出码 2 结束。 |
| `crates/voime/CORE.html` | §4.1 | 增 | 在 `run_ctl` 分流说明中补一句：Backend 分支先检查 endpoint，再写配置。 |

## 3. 其它技术上踩的坑

### P1. ONNX Runtime 在 GPU session 中会静默回退 CPU

1. **问题描述**

ORT 默认允许未分配到 GPU EP 的节点回退到 CPU。FireRed worker 即使注册 CUDA / MIGraphX / DirectML，也可能在 CPU 上执行部分算子，而 `Ready.runtime` 仍显示 GPU，导致性能与“GPU only”契约都被破坏，且很难从结果判断。

2. **解决方法**

- 创建 session 时显式调用 `with_disable_cpu_fallback()`；失败按 `load_failed` 处理。
- 进程只注册编译时选中的单一 GPU EP，不注册 CPU EP。
- 启动自检失败只让当前 backend 不可用；daemon 回退 CPU SenseVoice，不在 worker 内回退 CPU ORT。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-gpu-firered/PROVIDER.html` | §4.3 | 增 | 明确 session 必须禁用 CPU fallback，不能只依赖 EP 注册列表。 |
| `crates/voime-gpu-firered/CORE.html` | §2、§4.5 | 不动 | 已声明“不注册 CPU EP、不回退 CPU ORT”；PROVIDER 补充实现约束。 |

### P2. `llama-cpp-4` 0.11.0 的实际类型与文档草案签名有差异

1. **问题描述**

Qwen CORE 的草案签名使用较旧的 `LlamaContextParams` / `LlamaBatch` 路径，实际 0.11.0 的类型主要由 `prelude` 导出；BF16 判断是 `LlamaFtype::MostlyBF16`，`ftype()` 返回 `Option<LlamaFtype>`。此外默认 `n_ctx` 只有 512，30 s 音频加输出 token 很容易不够。

2. **解决方法**

- 统一使用 `llama_cpp_4::prelude::*`；版本固定 `=0.11.0`。
- 用 `matches!(model.ftype(), Some(LlamaFtype::MostlyBF16))` 判断主模型精度。
- 上下文固定 `n_ctx = 4096`、`n_batch = 2048`；解码每轮 `batch.clear()` 后只 add 一个新 token。
- 不启用采样温度 / top-p，使用 `LlamaSampler::greedy()`，遇 EOG 或 256 token 上限停止。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-gpu-qwen3/CORE.html` | §4.4、§4.6 | 改 | 用实际 0.11.0 类型路径、`MostlyBF16` 与 `n_ctx = 4096` 更新草案签名。 |
| `crates/voime-gpu-qwen3/PROVIDER.html` | §4.3 | 不动 | 已锁定 `llama-cpp-4` 0.11.0；类型细节归 CORE。 |

### P3. `tokenizers` 的 WordPiece 不会自动补 `[CLS]` / `[SEP]`

1. **问题描述**

FireRedPunc 导出的 ONNX 需要 BERT 风格的 `input_ids`，其中包含 `[CLS]` 101 与 `[SEP]` 102。只用 `WordPiece` + BERT normalizer / pre-tokenizer 时，`encode(text, true)` 并不会自动添加这两个特殊 token；同时 `get_offsets()` 返回的是原文字节偏移，不能按字符下标插入标点。

2. **解决方法**

- 在 `PuncTokenizer::encode` 中手动插入 101 / 102，并同步给 `lengths`。
- 分块按 tokenizer 的 token 边界切，不再按字符数切；每块重新编码后推理。
- `PuncLabels::insert` 使用 `Piece.end` 的字节偏移插入；跳过 label 0 与特殊 token。
- `[CLS]` / `[SEP]` 的 `Piece` 使用 0 / 文本长度作为插入边界，保证不会被当作正常标点位置。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-gpu-firered/CORE.html` | §4.13、§4.14 | 增 | 说明 WordPiece 需要手动补 `[CLS]` / `[SEP]`，分块按 token 边界，插入使用字节偏移。 |
| `crates/voime-gpu-firered/PROVIDER.html` | §4.2.1 | 不动 | 已定义输入包含特殊 token；编码细节归 CORE。 |

### P4. `rubato` 的流式重采样器会缓存不足一个块的数据

1. **问题描述**

`FastFixedIn` 只在 `pending` 达到固定块大小时产出样本；会话结束或设备块不足时，最后几十毫秒会留在重采样器内部。若只在 pipeline 侧清 `resample_pending`，这些样本不会进入 `FinishSegment`，导致硬切分尾部丢失；跨会话复用同一个重采样器还会把旧样本带进新会话。

2. **解决方法**

- 增加 `StreamResampler::flush()`，在 `FinishSegment` 的 `process_pending_audio(true)` 中用 `process_partial` 处理剩余块。
- `StartSegment`、`FinishSegment`、`SetMode` 与音频错误路径重建 / 丢弃 resampler，避免跨会话残留。
- pipeline 只保留自己的 `resample_pending`，并同时清空 resampler 内部状态。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-core/CORE.html` | §4.6、§5.3 | 增 | 说明 `FinishSegment` 必须 flush 重采样器；`SetMode` / Start 需要重建重采样器。 |
| `crates/voime-core/USER.html` | §4.4 | 不动 | 已定义硬切分排空音频；具体重采样器实现归 CORE。 |

### P5. Windows 登录启动项在安装器与托盘中使用不同名称

1. **问题描述**

`scripts/install-core.ps1` 最初写入 `HKCU\...\Run\Voime`，而 Windows 托盘 `Config::set_start_at_login` 写入 / 删除 `VoimeTray`。首次安装后会出现两个启动项；卸载只能删掉其中一个，关闭 `StartAtLogin` 也删不掉安装器写的那条。

2. **解决方法**

- 安装器与托盘统一使用 `Voime` 值名。
- 托盘保存配置后调用同一注册表写入函数；安装器卸载删除同名值。
- `Config::save` 保持临时文件 + 原子替换，避免写到一半留下损坏 JSON。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `frontends/windows/CORE.html` | §4.2 | 增 | 写明登录启动项固定使用 `HKCU\...\Run\Voime`，安装器与托盘共用同一个值名。 |
| `frontends/windows/USER.html` | §4.6 | 不动 | 已定义登录启动行为；值名细节归 CORE。 |

### P6. 独立 Cargo workspace 的产物路径与根 workspace 不同

1. **问题描述**

根 `cargo build` 只产出 `target/` 下的核心二进制；FireRed / Qwen3 与 Windows 前端是独立 workspace，产物分别在各自 crate 的 `target/`。安装脚本如果从根 `target/release` 找 worker 或托盘，会复制失败或复制到旧文件。

2. **解决方法**

- 安装脚本进入对应 crate 目录构建，并从该 crate 的 `target/release` 复制。
- GPU 预编译包优先从 `--source-dir/bin/` 读取；没有时才构建。
- Windows 安装从 `frontends/windows/target/release` 取 `voime-tray.exe`。
- 根测试只覆盖根 workspace；GPU / Windows 使用各自的 runner。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `CORE.html` | §1、§2 | 不动 | 已说明独立 workspace 与独立 Cargo.lock；安装脚本按本约束实现。 |
| `crates/voime-gpu-firered/README.html` | §2 | 不动 | 组件文档不需要重复根构建路径；BUILD 负责完整命令。 |

### P7. 音频错误回调使用 `try_send` 会在队列满时丢掉流错误哨兵

1. **问题描述**

音频数据回调按契约使用 `try_send` 丢弃当前块，避免阻塞设备线程；但错误回调如果也使用 `try_send`，在队列满时会丢掉零采样率哨兵，pipeline 永远不知道音频流已经失效，daemon 也就不会丢弃当前段或累计致命错误。

2. **解决方法**

- 错误回调改用阻塞 `send` 写哨兵；正常数据回调继续 `try_send`。
- 哨兵保持 `sample_rate = 0`、`channels = 0`、空样本，pipeline 用 `is_stream_error()` 识别。
- pipeline 对哨兵执行重置、取消 Preview、`Recording(false)` 与 `SegmentDropped`；连续达到上限后 `Fatal`。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-core/CORE.html` | §2、§5.6 | 增 | 明确数据回调用 `try_send`、错误回调用阻塞 `send` 写哨兵，哨兵不可丢。 |
| `crates/voime-core/PROVIDER.html` | §4.1 | 不动 | 已定义 cpal 错误回调；具体队列策略归 CORE。 |

### P8. DirectML 的线协议设备前缀是 `dml`，EP 名称却是 `directml`

1. **问题描述**

GPU common 的示例与 FireRed 文档使用 `dml:0` 作为 DirectML 设备标识，而 `OrtEp::Directml` 的显示字符串是 `directml`。直接用 EP 名称生成默认设备会得到 `directml:0`，worker 的 Hello 校验与 `Ready.device` 不一致；反过来只校验 `cuda` / `migraphx` / `dml` 又会漏掉构建 feature 与设备前缀的匹配。

2. **解决方法**

- `OrtEp` 分开 `as_str()`（provider 名称）与 `device_prefix()`（线协议前缀）：DirectML 的后者为 `dml`。
- `Runtime::load` 先检查请求设备前缀与编译时 EP 匹配，再解析设备序号。
- `Ready.device` 默认用 `device_prefix:index`，不直接拼 `as_str`。

3. **修订建议**

| 文档名 | 章节 | 动作 | 内容 |
|---|---|---|---|
| `crates/voime-gpu-firered/CORE.html` | §4.3、§4.6 | 增 | 区分 provider 名称 `directml` 与设备前缀 `dml`，并说明设备前缀必须匹配编译 EP。 |
| `crates/voime-gpu-firered/PROVIDER.html` | §4.3 | 不动 | 已按 EP 平台矩阵定义能力；命名映射归 CORE。 |
