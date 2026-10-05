# M0 / M1 测试记录

日期：2026-10-05。测试对象：`0.1.0-alpha.1`，实现代码现已收录于基线 `b3ff2ac`；下列测试在创建该提交前执行。

## 实际环境与结果

Windows、本机 Rust 1.96.0、Node.js 24.17.0、Playwright + Microsoft Edge。真实来源为 Codex CLI/runtime `0.160.0` 的本地桌面会话。

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --check` | 通过 |
| `cargo clippy --locked --all-targets -- -D warnings` | 通过 |
| `cargo test --locked` | 26 项通过：21 项采集/存储 + 4 项 HTTP + 1 项 CLI 集成测试 |
| `cargo build --locked` | 通过 |
| `npm run format:check` | 通过 |
| `npm run typecheck` | 通过 |
| `npm run build` | 通过，已更新随程序内嵌的 JavaScript |
| `npm run test:browser` | 6 项通过，使用真实浏览器；4 项真实服务流程、2 项控制 API 响应的并发/分页回归 |
| 当前项目真实日志接入 | 3 个匹配来源；初次保存 291 条记录，连续再次扫描新增为 0，无扫描错误 |
| 当前项目本地预览 | `http://127.0.0.1:4317` 已启动；状态接口确认 3 个匹配来源、无扫描错误 |
| OCR 自查后的真实数据库迁移 | 模式 1 升级至 2；保留 1 条压缩标记，其中包含加密内部内容/内部附加元数据字段的记录为 0 |

真实记录数量是当时的观测值；后续工作会继续产生记录。保存记录数包含必要元数据，既不等于界面可见条目数，也不等于工程问题数量。未将真实对话复制进测试材料或文档。

## 采集与存储覆盖

测试采用临时目录和人工构造的 JSONL 文件，验证对用户有意义的行为：

- 重复扫描和重启恢复不重复写入。
- UTF-8 中文被拆在未完成的一行中，续写后仍正确读取。
- 来源文件保持不变；排除同名前缀的邻近项目、接纳真正的子目录。
- 多会话交错写入仍保持独立读取进度。
- 坏行和未知类型可见，后续正常记录继续采集。
- 同长度重写和文件截断均保留旧证据，开启新代次。
- 工具结果补到已有条目，重启后增量游标仍返回修订。
- 相同命令的不同调用保留为不同尝试。
- 同一助手消息的多种表示合并并保留对应来源。
- 后到的匹配用户表示替换临近低优先级表示；混合格式会话及真实重复输入不会整批被隐藏。
- 已知内部指令与推理不进入保存证据。
- 命令退出与回合完成状态不表示问题已经解决。
- 历史分页和新增分页不遗漏已有样例；新用户消息不错误归入上一回合。
- 数据库不能作为另一个项目的数据目录；来源缺失和会话头无效产生可见错误。
- 压缩记录只保存白名单字段；旧模式数据库迁移后不含这些内部字段，历史 ID 与采集进度保留。
- 原生记录明确标记失败时，即使没有退出码，也纳入失败计数。

CLI 测试实际运行程序，验证 `--once` 遇到扫描错误时仍输出有效 JSON，但返回非零退出码；错误来源移除后恢复成功退出。

## 接口与浏览器覆盖

HTTP 测试验证网页资源、安全响应头、外部 Host/Origin 拒绝、未知证据 404、互斥分页参数 400、写入方法 405。

浏览器测试验证：

1. 自动发现会话，显示失败记录，点击准确定位到来源第 4 行，筛选正常。
2. 向来源追加内容后无需刷新页面即出现；类似 HTML 注入的文本按字面显示，未创建图片节点或执行脚本。
3. 一个仍在等待结果的调用收到后续结果后原位更新，条目数不增加。
4. 390 像素窄屏无横向溢出；接口断线后显示状态，保留已加载内容。
5. 旧事件收到迟到结果后，历史分页边界仍指向连续加载的历史，不跳过中间记录。
6. 控制历史页晚于实时更新返回，验证旧修订不能覆盖较新的结果。

OCR 自查新增的 4 项 Rust 与 2 项浏览器测试均先在原实现上复现失败，再在修复后通过；详见 [审查报告](ocr-review.md)。

已检查 1440 × 1080 桌面和 390 × 844 窄屏截图。截图位于被 Git 忽略的 `test-results/demine-desktop.png` 与 `test-results/demine-mobile.png`，内容均为人工构造案例。

## 复现

在仓库目录执行：

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

Windows 浏览器测试默认调用已安装的 Edge；Linux 需先执行 `npx playwright install --with-deps chromium`。测试服务只使用 `target/` 下生成的独立假数据和数据库，端口为 4318，不读取真实 Codex 会话。

当前执行环境对 Windows 路径解析和 Rust 工具链存在沙箱限制；验证经自动审批后在沙箱外执行。普通沙箱中的一次真实接入因此报过“拒绝访问”，随后在可访问相同目录的执行环境中复验通过。

本轮另启动了隐藏窗口的本地预览进程；PID 与输出保存在被 Git 忽略的 `.demine/preview.pid`、`preview.stdout.log`、`preview.stderr.log`，未配置开机自启。需要停止时，在仓库目录核对并终止该预览进程：

```powershell
$previewProcess = Get-Process -Id (Get-Content .demine/preview.pid) -ErrorAction SilentlyContinue
$expectedPreview = (Resolve-Path -LiteralPath .\target\debug\demine.exe).Path
if ($previewProcess -and $previewProcess.Path -eq $expectedPreview) {
    Stop-Process -Id $previewProcess.Id
}
```

PID 可能在进程退出后被系统复用，因此先核对路径；正常前台启动仍使用 `Ctrl+C` 停止。修改程序并重新编译前，也需停止此预览，避免 Windows 锁定可执行文件。

## 尚未验证

- GitHub Actions 只完成配置，未推送执行；Linux、其他浏览器及其他 Codex 版本未完成实测。
- 未执行随机进程崩溃/断电注入；检查点原子性基于 SQLite 事务，不能将重启测试称为完整崩溃恢复测试。
- 未做大规模历史目录、超大单行、长期运行、并发多进程、文件迁移与磁盘耗尽压测。
- 没有评估 AI 经验提取质量，因为该功能尚未实现。
