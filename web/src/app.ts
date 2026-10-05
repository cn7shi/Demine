interface Session {
  id: string;
  title: string;
  cwd: string;
  cli_version: string;
  updated_at: string;
  event_count: number;
  failed_count: number;
  warning_count: number;
}
interface Entry {
  id: number;
  kind: string;
  title: string;
  input: string;
  output: string;
  status: string;
  timestamp: string;
  turn_id: string | null;
  call_id: string | null;
  generation: number;
  revision: number;
}
interface Page {
  events: Entry[];
  cursor: number;
  has_more: boolean;
  hidden_ids: number[];
}
interface Status {
  project: string;
  sessions_root: string;
  poll_seconds: number;
  version: string;
  scan: {
    matched_files: number;
    saved_records: number;
    rewritten_files: number;
    pending_lines: number;
    errors: string[];
    finished_at_ms: number;
  };
}
interface Evidence {
  source: string;
  generation: number;
  line: number;
  byte_offset: number;
  record_type: string;
  raw: string;
}

function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (!found) throw new Error(`Missing element: ${id}`);
  return found as T;
}
function node<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className: string,
  content?: string,
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  el.className = className;
  if (content !== undefined) el.textContent = content;
  return el;
}
async function get<T>(path: string): Promise<T> {
  const response = await fetch(path, {
    cache: "no-store",
    signal: AbortSignal.timeout(10000),
  });
  if (!response.ok) throw new Error(`请求失败 (${response.status})`);
  return response.json() as Promise<T>;
}
function date(value: string): string {
  const time = new Date(value);
  return Number.isNaN(time.valueOf())
    ? "时间未知"
    : time.toLocaleTimeString("zh-CN", {
        hour: "2-digit",
        minute: "2-digit",
        second: "2-digit",
      });
}
const entries = new Map<number, Entry>();
let currentSession: string | null = null,
  cursor = 0,
  historyBefore: number | null = null,
  selected: number | null = null,
  filter = "all",
  busy = false,
  more = false,
  sessionSignature = "",
  evidenceRevision = -1;
let selectionGeneration = 0;
const labels: Record<string, string> = {
  started: "进行中",
  returned: "已收到返回",
  completed: "执行结束",
  succeeded: "退出码为 0",
  failed: "明确执行失败",
  interrupted: "已中断",
  unknown: "待识别",
  recorded: "已记录",
};
const icons: Record<string, string> = {
  user: "你",
  assistant: "↗",
  command: ">_",
  tool: "◇",
  file: "▤",
  turn: "·",
  warning: "!",
  unknown: "?",
  context: "↻",
  transport: "{}",
};

function setError(message: string) {
  const target = element("error");
  target.textContent = message;
  target.hidden = !message;
}
function merge(page: Page) {
  for (const id of page.hidden_ids) entries.delete(id);
  for (const entry of page.events) {
    // A history request may finish after polling has already received a result.
    const previous = entries.get(entry.id);
    if (!previous || entry.revision >= previous.revision)
      entries.set(entry.id, entry);
  }
}

function renderTimeline() {
  const parent = element("timeline");
  const search = element<HTMLInputElement>("search").value.trim().toLowerCase();
  const showTransport = element<HTMLInputElement>("show-transport").checked;
  const visible = [...entries.values()]
    .sort((a, b) => a.id - b.id)
    .filter((e) => {
      if (e.kind === "transport" && !showTransport) return false;
      if (
        filter === "operations" &&
        !["command", "tool", "file", "transport"].includes(e.kind)
      )
        return false;
      if (filter === "messages" && !["user", "assistant"].includes(e.kind))
        return false;
      if (
        filter === "attention" &&
        e.status !== "failed" &&
        !["warning", "unknown"].includes(e.kind)
      )
        return false;
      return (
        !search ||
        `${e.title}\n${e.input}\n${e.output}`.toLowerCase().includes(search)
      );
    });
  element("event-count").textContent = `已加载 ${entries.size} 条`;
  element("older").hidden = !more;
  const keep = new Set(visible.map((e) => String(e.id)));
  for (const child of [...parent.children])
    if (!keep.has((child as HTMLElement).dataset.id ?? "")) child.remove();
  if (!visible.length) {
    parent.replaceChildren(
      node(
        "div",
        "empty-state",
        entries.size
          ? "当前条件下没有记录。可更换筛选或加载更早的记录。"
          : "在这个项目中使用 Codex，执行与交流记录会自动出现在这里。",
      ),
    );
    return;
  }
  for (const entry of visible) {
    let article = parent.querySelector<HTMLElement>(`[data-id="${entry.id}"]`);
    if (article?.dataset.revision === String(entry.revision)) {
      article.classList.toggle("selected", entry.id === selected);
      continue;
    }
    const expanded = article?.querySelector("details")?.open ?? false;
    const fresh = node(
      "article",
      `event ${entry.status} ${entry.kind}${entry.id === selected ? " selected" : ""}`,
    );
    fresh.dataset.id = String(entry.id);
    fresh.dataset.revision = String(entry.revision);
    const button = node("button", "event-main");
    button.type = "button";
    button.setAttribute("aria-label", `${entry.title}，查看证据`);
    button.append(node("span", "event-icon", icons[entry.kind] ?? "?"));
    const body = node("div", "");
    const meta = node("div", "event-meta");
    meta.append(
      node("span", "event-title", entry.title),
      node("time", "event-time", date(entry.timestamp)),
    );
    body.append(meta);
    const preview = entry.input || entry.output;
    body.append(node("div", "event-preview", preview.slice(0, 300)));
    body.append(
      node(
        "span",
        `badge ${entry.status}`,
        `${labels[entry.status] ?? entry.status}${entry.generation > 0 ? ` · 来源第 ${entry.generation + 1} 代` : ""}`,
      ),
    );
    button.append(body);
    button.addEventListener("click", () => {
      selected = entry.id;
      renderTimeline();
      void showEvidence(entry);
    });
    fresh.append(button);
    const details = node("details", "");
    details.open = expanded;
    details.append(node("summary", "", "展开内容"));
    if (entry.input) {
      details.append(
        node("p", "", "请求 / 操作"),
        node("pre", "", entry.input),
      );
    }
    if (entry.output) {
      details.append(
        node("p", "", "回复 / 观察"),
        node("pre", "", entry.output),
      );
    }
    if (entry.turn_id) details.append(node("p", "", `回合 ${entry.turn_id}`));
    fresh.append(details);
    if (article) article.replaceWith(fresh);
    else parent.append(fresh);
  }
  // Older pages arrive after newer nodes. Reorder nodes without replacing open details.
  for (const entry of visible) {
    const child = parent.querySelector(`[data-id="${entry.id}"]`);
    if (child) parent.append(child);
  }
}

async function showEvidence(entry: Entry) {
  const generation = selectionGeneration;
  const target = element("evidence");
  target.replaceChildren(node("p", "evidence-note", "正在定位原始证据…"));
  element("close-evidence").hidden = false;
  try {
    const rows = await get<Evidence[]>(`/api/events/${entry.id}/evidence`);
    if (generation !== selectionGeneration || selected !== entry.id) return;
    evidenceRevision = entry.revision;
    target.replaceChildren(
      node(
        "p",
        "evidence-note",
        `以下是“${entry.title}”对应的记录。相同操作的请求与返回可能来自不同行。`,
      ),
    );
    for (const row of rows) {
      const card = node("section", "evidence-card");
      card.append(
        node(
          "div",
          "evidence-location",
          `${row.source}\n第 ${row.line} 行 · 字节 ${row.byte_offset} · 来源第 ${row.generation + 1} 代`,
        ),
      );
      let raw = row.raw;
      try {
        raw = JSON.stringify(JSON.parse(raw), null, 2);
      } catch {
        /* Malformed source must still be readable. */
      }
      card.append(node("pre", "", raw));
      target.append(card);
    }
  } catch (error) {
    if (generation === selectionGeneration && selected === entry.id)
      target.replaceChildren(
        node("p", "evidence-note", `证据暂时无法读取。${String(error)}`),
      );
  }
}

async function selectSession(session: Session) {
  currentSession = session.id;
  selectionGeneration++;
  const generation = selectionGeneration;
  cursor = 0;
  historyBefore = null;
  selected = null;
  evidenceRevision = -1;
  entries.clear();
  more = false;
  element("session-title").textContent = session.title;
  element("evidence").replaceChildren(
    node("div", "evidence-empty", "选择一条记录，查看原始证据。"),
  );
  element("close-evidence").hidden = true;
  for (const button of document.querySelectorAll<HTMLElement>(
    ".session-button",
  ))
    button.classList.toggle("selected", button.dataset.session === session.id);
  renderTimeline();
  try {
    const page = await get<Page>(
      `/api/sessions/${encodeURIComponent(session.id)}/events`,
    );
    if (generation !== selectionGeneration) return;
    merge(page);
    cursor = Math.max(cursor, page.cursor);
    historyBefore = page.events[0]?.id ?? null;
    more = page.has_more;
    renderTimeline();
  } catch (error) {
    if (generation === selectionGeneration)
      setError(`无法打开会话。${String(error)}`);
  }
}

function renderSessions(sessions: Session[]) {
  const signature = JSON.stringify(sessions);
  if (signature === sessionSignature) return;
  sessionSignature = signature;
  const parent = element("sessions");
  parent.replaceChildren();
  element("session-count").textContent = String(sessions.length);
  for (const session of sessions) {
    const button = node(
      "button",
      `session-button${session.id === currentSession ? " selected" : ""}`,
    );
    button.type = "button";
    button.dataset.session = session.id;
    button.append(
      node("strong", "", session.title),
      node(
        "small",
        "",
        `${session.event_count} 条记录 · ${session.failed_count} 次执行失败`,
      ),
    );
    button.addEventListener("click", () => void selectSession(session));
    parent.append(button);
  }
  if (!sessions.length)
    parent.append(node("p", "sidebar-empty", "等待第一段工作记录"));
}

async function refresh() {
  if (busy) return;
  busy = true;
  element<HTMLButtonElement>("refresh").disabled = true;
  try {
    const [status, sessions] = await Promise.all([
      get<Status>("/api/status"),
      get<Session[]>("/api/sessions"),
    ]);
    element("project-name").textContent =
      status.project.split(/[\\/]/).filter(Boolean).at(-1) ?? status.project;
    element("project-path").textContent = status.project;
    const scanned = status.scan.finished_at_ms > 0;
    const stale =
      scanned &&
      Date.now() - status.scan.finished_at_ms >
        Math.max(15000, status.poll_seconds * 5000);
    element("connection").textContent = !scanned
      ? "首次同步中"
      : stale
        ? "同步耗时较长"
        : status.scan.errors.length
          ? "采集需留意"
          : "本地观察中";
    element("connection-dot").classList.toggle(
      "offline",
      stale || status.scan.errors.length > 0,
    );
    element("updated").textContent = scanned
      ? `最近同步 ${date(new Date(status.scan.finished_at_ms).toISOString())}`
      : "首次同步中";
    element("stat-events").textContent = String(
      sessions.reduce((n, s) => n + s.event_count, 0),
    );
    element("stat-failures").textContent = String(
      sessions.reduce((n, s) => n + s.failed_count, 0),
    );
    element("stat-unknown").textContent = String(
      sessions.reduce((n, s) => n + s.warning_count, 0),
    );
    setError(status.scan.errors.join("\n"));
    const notice = element("notice");
    notice.textContent = status.scan.pending_lines
      ? "正在等待尚未写完的记录；已保存的内容不受影响。"
      : status.scan.rewritten_files
        ? "检测到来源文件变化，已保留旧历史并开始记录新的来源代次。"
        : "";
    notice.hidden = !notice.textContent;
    renderSessions(sessions);
    if (!currentSession && sessions[0]) {
      await selectSession(sessions[0]);
    } else if (currentSession) {
      const generation = selectionGeneration;
      const page = await get<Page>(
        `/api/sessions/${encodeURIComponent(currentSession)}/events?after=${cursor}`,
      );
      if (generation === selectionGeneration) {
        merge(page);
        cursor = Math.max(cursor, page.cursor);
        renderTimeline();
        const entry = selected === null ? undefined : entries.get(selected);
        if (entry && entry.revision !== evidenceRevision)
          await showEvidence(entry);
      }
    }
  } catch (error) {
    element("connection").textContent = "连接已断开";
    element("connection-dot").classList.add("offline");
    setError(`暂时连接不到本地服务，已显示的记录仍然保留。${String(error)}`);
  } finally {
    busy = false;
    element<HTMLButtonElement>("refresh").disabled = false;
  }
}

element("refresh").addEventListener("click", () => void refresh());
element("search").addEventListener("input", renderTimeline);
element("show-transport").addEventListener("change", renderTimeline);
for (const button of document.querySelectorAll<HTMLButtonElement>(
  "[data-filter]",
))
  button.addEventListener("click", () => {
    filter = button.dataset.filter ?? "all";
    for (const sibling of document.querySelectorAll("[data-filter]")) {
      const active = sibling === button;
      sibling.classList.toggle("active", active);
      sibling.setAttribute("aria-pressed", String(active));
    }
    renderTimeline();
  });
element("close-evidence").addEventListener("click", () => {
  selected = null;
  evidenceRevision = -1;
  element("evidence").replaceChildren(
    node("div", "evidence-empty", "选择一条记录，查看原始证据。"),
  );
  element("close-evidence").hidden = true;
  renderTimeline();
});
element("older").addEventListener("click", async () => {
  if (!currentSession || historyBefore === null) return;
  const button = element<HTMLButtonElement>("older");
  button.disabled = true;
  const generation = selectionGeneration;
  try {
    // Incremental updates may mention much older IDs, leaving a gap in loaded
    // history. Only history pages can move this contiguous pagination boundary.
    const before = historyBefore;
    const page = await get<Page>(
      `/api/sessions/${encodeURIComponent(currentSession)}/events?before=${before}`,
    );
    if (generation !== selectionGeneration) return;
    merge(page);
    historyBefore = page.events[0]?.id ?? historyBefore;
    more = page.has_more;
    renderTimeline();
  } catch (error) {
    setError(`无法读取更早的记录。${String(error)}`);
  } finally {
    button.disabled = false;
  }
});
void refresh();
window.setInterval(() => void refresh(), 2000);
