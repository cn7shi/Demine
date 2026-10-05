// Real server + synthetic transcripts. Never feed personal sessions to browser tests.
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { resolve, join } from "node:path";
import { spawn } from "node:child_process";

const target = resolve("target");
mkdirSync(target, { recursive: true });
const root = mkdtempSync(join(target, "browser-fixture-"));
const project = join(root, "屏幕适配项目");
const sessions = join(root, "sessions");
mkdirSync(project, { recursive: true });
mkdirSync(sessions, { recursive: true });
const log = join(sessions, "session.jsonl");
const row = (payload, type = "event_msg") => ({
  timestamp: "2026-10-05T11:30:00Z",
  type,
  payload,
});
const item = (value) =>
  row({ type: "item_completed", turn_id: "turn-fixture", item: value });
const records = [
  row(
    { id: "fixture-session", cwd: project, cli_version: "0.160.0" },
    "session_meta",
  ),
  item({ type: "UserMessage", id: "u1", content: "检查小屏设备上的布局偏移" }),
  row({ type: "task_started", turn_id: "turn-fixture" }),
  item({
    type: "CommandExecution",
    id: "c1",
    command: "run layout-check --small-screen",
    exit_code: 1,
    aggregated_output:
      "Expected panel to stay inside viewport. Observed horizontal overflow.",
  }),
  item({
    type: "AgentMessage",
    id: "a1",
    content: "小屏检查未通过。接下来缩小范围，检查容器宽度约束。",
  }),
  item({
    type: "FileChange",
    id: "f1",
    changes: [{ path: "src/layout.css", summary: "Update width constraint" }],
  }),
  item({
    type: "CommandExecution",
    id: "c2",
    command: "run layout-check --small-screen",
    exit_code: 0,
    aggregated_output:
      "Small-screen check passed. Other device sizes were not checked.",
  }),
  row({ type: "task_complete", turn_id: "turn-fixture" }),
  { type: "future_event", payload: { name: "future-compatible-record" } },
];
writeFileSync(log, records.map((r) => JSON.stringify(r)).join("\n") + "\n");
writeFileSync(
  join(target, "browser-control.json"),
  JSON.stringify({ root, project, sessions, log }),
);
const executable = join(
  target,
  "debug",
  process.platform === "win32" ? "demine.exe" : "demine",
);
const child = spawn(
  executable,
  [
    "--project",
    project,
    "--sessions-dir",
    sessions,
    "--data-dir",
    join(root, "data"),
    "--port",
    "4318",
    "--poll-seconds",
    "1",
  ],
  { stdio: "inherit", windowsHide: true },
);
child.on("error", (error) => {
  console.error(error.message);
  process.exitCode = 1;
});
child.on("exit", (code) => {
  process.exit(code ?? 0);
});
for (const signal of ["SIGINT", "SIGTERM"])
  process.on(signal, () => child.kill(signal));
process.on("exit", () => {
  if (child.exitCode === null) child.kill();
});
