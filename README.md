# Demine

**开发者的隐性工程经验沉淀外脑。**

接入你正在使用的 Agent 工具，自动留下被最终结果带过的尝试、失败和修正，让你按需展开理解，逐步积累自己的工程经验。

从 IDE 到 Agent-first，开发者的注意力逐渐从具体执行转向任务与结果。AI 仍会在真实工程里反复试错，而一句“完成了”往往不足以让人了解它遇到的约束。Demine 希望让这些经历进入人的视野。

## 当前进度：M0 / M1 本地预览

目前可以自动采集 **指定项目的 Codex 本地执行记录**，并在浏览器里回放。

- 自动发现会话、持续读取新增记录，无需手动导出。
- 保存消息、工具请求与返回、原生命令执行、文件改动及回合事件。
- 重启续读与去重；等待半行写完；提示坏行和未知类型。
- 检测来源截断/已读内容变化，保留旧历史并增加来源代次。
- 按会话查看时间线，展开内容，定位原始证据；支持类型筛选、已加载内容搜索和更早记录分页。
- 本地运行，无账号、无模型 API 调用、无记录上传。

**尚未实现**：自动识别工程暗坑、因果分析、问题归组、个人笔记、知识库、MCP、桌面安装包。当前界面呈现执行证据，不能将“执行结束”理解为“问题已验证解决”。

## 启动

需要 Rust 稳定工具链。本机已用 Rust 1.96 / Windows 验证。网页资源随程序编译，运行时不需要 Node.js。

在仓库目录运行：

```powershell
cargo run --locked -- --project "E:\MyProject\Rust\Demine"
```

打开 [本地界面](http://127.0.0.1:4317)。照常在指定项目中使用 Codex，新增记录会自动出现；按 `Ctrl+C` 停止服务。

第一次启动会补齐来源目录内已经存在、且工作目录属于当前项目或其子目录的会话。项目选择目前通过启动参数完成；界面展示接入状态，没有图形化目录选择器。

默认读取 `$CODEX_HOME/sessions`，未设置时读取用户目录下 `.codex/sessions`。数据库写到目标项目的 `.demine/demine.sqlite3`。来源文件只读；数据目录已在本仓库的 `.gitignore` 中排除。观察其他仓库时，也应在该仓库忽略数据目录，或使用外部 `--data-dir`。

可选参数：

```powershell
# 使用自定义来源与独立数据目录
cargo run --locked -- --project "E:\YourProject" --sessions-dir "C:\Users\you\.codex\sessions" --data-dir "E:\DemineData\YourProject" --port 4317

# 仅执行一次本地扫描，输出计数，不打印对话正文
cargo run --locked -- --project . --once
```

`--poll-seconds` 可设置 1–60 秒，默认 2 秒。程序仅监听 `127.0.0.1`；只提供读取接口。没有接管或修改 Codex 配置。

`--once` 出现扫描错误时会在 JSON 报告中列出，并返回非零退出码。旧的本地数据库会自动迁移到当前模式，保留历史与采集进度。

## 兼容与证据边界

- 本机真实接入样本：Codex CLI/runtime `0.160.0` 产生的桌面会话记录。
- 支持 `response_item`、部分 `event_msg` 及原生 `item_completed` 表示；其他版本只做容错兼容，尚未逐一实测。
- 不导入隐藏推理正文/密文和系统、开发者指令；会话元数据与回合上下文只保存必要定位字段。
- 未保存到来源中的截图、设备状态、历史代码状态和工具输出无法凭空恢复。
- 不会执行记录中的命令。来源文本在网页中作为普通文本呈现。
- 日志格式可能变化，解析适配独立于存储与界面；参考 [Codex 官方格式说明](https://learn.chatgpt.com/docs/hooks)。

大规模历史目录、超大单行记录、归档移动去重、云会话以及多进程同时采集尚未作为本版支持场景。详情见 [实现记录](docs/implementation.md)。

## 开发与检查

后端使用 Rust + SQLite，前端使用 TypeScript。本地服务内嵌网页；修改 `web/src/app.ts` 后应重新生成 JavaScript，再重新编译 Rust。

```powershell
npm ci --ignore-scripts
npm run format:check
npm run typecheck
npm run build
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
npm run test:browser
```

浏览器测试在 Windows 默认使用已安装的 Edge；Linux 使用 Playwright Chromium，需要先运行 `npx playwright install --with-deps chromium`。测试使用人工构造的本地记录，不访问真实 Codex 会话。

仓库已添加 Windows / Linux Rust 检查及 Linux 浏览器测试的 GitHub Actions 配置；云端结果需要推送后验证。

## 项目约定

- [工程档案总览](docs/README.md)：按问题查找实现、逻辑、决策与验证材料。
- [工程规范](AGENTS.md)：后续人与 AI 修改仓库时共同遵守的约定。
- [开发履历](docs/development-log.md)：每批写了什么、为何修改、怎样验证。
- [关键逻辑](docs/logic.md)：数据流、状态变化、异常处理和代码/测试入口。
- [决策记录](docs/decisions.md)：为什么采用这些实现，以及代价。
- [项目路线](docs/roadmap.md)：里程碑与验收条件。
- [实现记录](docs/implementation.md)：已经做了什么、边界与待办。
- [测试记录](docs/testing.md)：实际执行的检查和结果。
- [OCR 自查报告](docs/ocr-review.md)：审查方式、可复现问题及修复结果。

开源许可证尚待项目维护者选择；当前没有声明授权许可证。
