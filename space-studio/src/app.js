/* Space Studio — 前端逻辑（Android Studio 风格 IDE）
   Tauri invoke 调用 Rust 命令；无 Tauri 时退化为 mock（浏览器预览）。
   所有异常落到"输出"工具窗口，绝不静默失败。 */

"use strict";

/* ---------- Tauri 桥 ---------- */
const isTauri =
  typeof window.__TAURI__ !== "undefined" ||
  typeof window.__TAURI_INTERNALS__ !== "undefined";
const invoke = (cmd, args) => {
  if (isTauri && typeof window.__TAURI__ !== "undefined") {
    return window.__TAURI__.core.invoke(cmd, args);
  }
  return mockInvoke(cmd, args);
};

window.addEventListener("error", (e) => {
  logSafe(`[JS错误] ${e.message} @${e.filename || ""}:${e.lineno || ""}`);
});
window.addEventListener("unhandledrejection", (e) => {
  logSafe(`[异步错误] ${e.reason && e.reason.message ? e.reason.message : e.reason}`);
});
function logSafe(msg) {
  const el = document.getElementById("log");
  if (el) {
    const line = document.createElement("div");
    line.className = "err";
    line.textContent = `> ${msg}`;
    el.appendChild(line);
    el.scrollTop = el.scrollHeight;
  }
}

/* ---------- 状态 ---------- */
const state = {
  root: "",
  projects: [],
  current: null,            // 当前工程路径
  currentName: "",
  files: [],                // 文件树（相对路径，目录以 / 结尾）
  expanded: new Set(),      // 展开的目录
  openTabs: [],             // 打开的文件 tab
  activeTab: null,          // 当前激活 tab 的 rel
};

/* ---------- 深空壁纸 ---------- */
function drawWallpaper() {
  const cv = document.getElementById("wallpaper");
  if (!cv) return;
  const ctx = cv.getContext("2d");
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  cv.width = window.innerWidth * dpr;
  cv.height = window.innerHeight * dpr;
  ctx.scale(dpr, dpr);
  const w = window.innerWidth, h = window.innerHeight;
  const g = ctx.createLinearGradient(0, 0, 0, h);
  g.addColorStop(0, "#0B0618");
  g.addColorStop(1, "#1A0F2E");
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, w, h);
  let seed = 20260926;
  const rnd = () => (seed = (seed * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff;
  for (let gy = 0; gy < h; gy += 128) {
    for (let gx = 0; gx < w; gx += 128) {
      for (let i = 0; i < 3; i++) {
        const x = gx + rnd() * 128, y = gy + rnd() * 128;
        const r = rnd() < 0.85 ? 1 : 1.6;
        const a = 0.15 + rnd() * 0.35;
        ctx.fillStyle = `rgba(203,184,240,${a})`;
        ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2); ctx.fill();
      }
    }
  }
  const planet = (cx, cy, r, color) => {
    ctx.strokeStyle = color; ctx.fillStyle = color;
    ctx.lineWidth = 1;
    ctx.beginPath(); ctx.arc(cx, cy, r, 0, Math.PI * 2); ctx.stroke();
    ctx.beginPath(); ctx.arc(cx, cy, r * 0.32, 0, Math.PI * 2); ctx.fill();
    ctx.save(); ctx.translate(cx, cy); ctx.rotate(-0.3);
    ctx.beginPath(); ctx.ellipse(0, 0, r * 2.6, r * 0.9, 0, 0, Math.PI * 2); ctx.stroke();
    ctx.restore();
  };
  ctx.globalAlpha = 0.5;
  planet(w * 0.85, h * 0.14, 30, "rgba(124,92,255,0.7)");
  planet(w * 0.08, h * 0.8, 20, "rgba(255,149,0,0.5)");
  ctx.globalAlpha = 1;
}
window.addEventListener("resize", drawWallpaper);

/* ---------- 日志 ---------- */
function log(msg, cls) {
  const el = document.getElementById("log");
  if (!el) return;
  const line = document.createElement("div");
  if (cls) line.className = cls;
  line.textContent = `> ${msg}`;
  el.appendChild(line);
  el.scrollTop = el.scrollHeight;
}

/* ---------- 工程列表 ---------- */
async function refreshProjects() {
  try {
    state.projects = await invoke("list_projects", { root: state.root });
    if (state.projects.length > 0 && !state.current) {
      await openProject(state.projects[0]);
    }
    updateEmptyHint();
  } catch (e) { log("列出工程失败: " + e, "err"); }
}

function updateEmptyHint() {
  document.getElementById("empty-hint").classList.toggle("hidden", state.current !== null);
}

/* ---------- 打开工程：加载文件树 + 默认打开源码 ---------- */
async function openProject(p) {
  state.current = p.path;
  state.currentName = p.name;
  state.openTabs = [];
  state.activeTab = null;
  state.expanded = new Set([""]);
  document.getElementById("tb-project-name").textContent = p.name;
  document.getElementById("code-area").value = "";
  updateLineNums();
  updateEmptyHint();
  try {
    state.files = await invoke("list_files", { projectRoot: p.path });
    renderFileTree();
    log(`打开工程 ${p.name}（${state.files.length} 项）`);
    if (state.files.includes("src/main.rs")) {
      openFile("src/main.rs");
    } else if (state.files.includes("main.rs")) {
      openFile("main.rs");
    } else if (state.files.some(f => !f.endsWith("/"))) {
      openFile(state.files.find(f => !f.endsWith("/")));
    }
  } catch (e) {
    log("加载文件树失败: " + e, "err");
  }
}

/* ---------- 文件树渲染 ---------- */
function renderFileTree() {
  const ul = document.getElementById("file-tree");
  ul.innerHTML = "";
  if (!state.current) {
    document.getElementById("empty-hint").classList.remove("hidden");
    return;
  }
  document.getElementById("empty-hint").classList.add("hidden");

  for (const rel of state.files) {
    const isDir = rel.endsWith("/");
    const path = isDir ? rel.slice(0, -1) : rel;
    const parts = path.split("/");
    const parentPath = isDir ? path : parts.slice(0, -1).join("/");
    const depth = parentPath === "" ? 0 : parentPath.split("/").length;
    if (!isDir && parentPath !== "" && !state.expanded.has(parentPath)) continue;
    if (isDir && depth > 0) {
      const grand = parts.slice(0, -1).join("/");
      if (grand !== "" && !state.expanded.has(grand)) continue;
    }
    const name = parts[parts.length - 1];
    const li = document.createElement("li");
    const node = document.createElement("div");
    node.className = "file-node" + (isDir ? " dir" : "") + (state.activeTab === rel ? " active" : "");
    node.style.paddingLeft = (8 + depth * 14) + "px";

    if (isDir) {
      const expanded = state.expanded.has(path);
      node.innerHTML = `
        <span class="arrow">${expanded ? "▾" : "▸"}</span>
        <svg class="icon" viewBox="0 0 24 24"><path d="M3 6a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" fill="none" stroke="currentColor" stroke-width="1.6"/></svg>
        <span class="name">${escapeHtml(name)}</span>`;
      node.onclick = () => toggleDir(path);
    } else {
      const ext = name.includes(".") ? name.split(".").pop() : "";
      const color = { rs: "#E37A5C", json: "#D9C27A", kir: "#7C5CFF" }[ext] || "currentColor";
      node.innerHTML = `
        <span class="arrow"></span>
        <svg class="icon" viewBox="0 0 24 24"><path d="M14 3H6a1 1 0 0 0-1 1v16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8z" fill="none" stroke="${color}" stroke-width="1.6" stroke-linejoin="round"/><path d="M14 3v5h5" fill="none" stroke="${color}" stroke-width="1.6"/></svg>
        <span class="name">${escapeHtml(name)}</span>`;
      node.onclick = () => openFile(rel);
    }
    li.appendChild(node);
    ul.appendChild(li);
  }
}

function toggleDir(path) {
  if (state.expanded.has(path)) state.expanded.delete(path);
  else state.expanded.add(path);
  renderFileTree();
}

/* ---------- 文件 tab ---------- */
async function openFile(rel) {
  if (!state.current) return;
  if (!state.openTabs.includes(rel)) {
    state.openTabs.push(rel);
  }
  state.activeTab = rel;
  renderTabs();
  renderFileTree();
  try {
    const content = await invoke("read_file", { projectRoot: state.current, rel });
    document.getElementById("code-area").value = content;
    updateLineNums();
    log(`打开 ${rel}`);
  } catch (e) {
    document.getElementById("code-area").value = "";
    log("读取失败: " + e, "err");
  }
}

function closeTab(rel) {
  const i = state.openTabs.indexOf(rel);
  if (i < 0) return;
  state.openTabs.splice(i, 1);
  if (state.activeTab === rel) {
    state.activeTab = state.openTabs.length ? state.openTabs[Math.max(0, i - 1)] : null;
    if (state.activeTab) openFile(state.activeTab);
    else { document.getElementById("code-area").value = ""; updateLineNums(); }
  }
  renderTabs();
  renderFileTree();
}

function renderTabs() {
  const bar = document.getElementById("editor-tabs");
  bar.innerHTML = "";
  for (const rel of state.openTabs) {
    const tab = document.createElement("div");
    tab.className = "editor-tab" + (rel === state.activeTab ? " active" : "");
    const name = rel.split("/").pop();
    tab.innerHTML = `<span>${escapeHtml(name)}</span><span class="close">×</span>`;
    tab.onclick = () => openFile(rel);
    tab.querySelector(".close").onclick = (e) => { e.stopPropagation(); closeTab(rel); };
    bar.appendChild(tab);
  }
}

/* ---------- 编辑器 ---------- */
function updateLineNums() {
  const area = document.getElementById("code-area");
  const nums = document.getElementById("line-nums");
  if (!area || !nums) return;
  const n = area.value.split("\n").length;
  let s = "";
  for (let i = 1; i <= n; i++) s += i + "\n";
  nums.textContent = s;
}

async function saveFile() {
  if (!state.activeTab) return log("请先打开一个文件", "warn");
  try {
    await invoke("write_file", {
      projectRoot: state.current,
      rel: state.activeTab,
      content: document.getElementById("code-area").value,
    });
    log("已保存 " + state.activeTab);
  } catch (e) { log("保存失败: " + e, "err"); }
}

/* ---------- 新建工程（模态框） ---------- */
function showModal() {
  document.getElementById("modal").classList.remove("hidden");
  const input = document.getElementById("modal-input");
  input.value = "";
  input.focus();
}
function hideModal() { document.getElementById("modal").classList.add("hidden"); }

async function createProject() {
  const name = document.getElementById("modal-input").value.trim();
  hideModal();
  if (!name) return;
  try {
    const p = await invoke("new_project", { root: state.root, name });
    state.projects = await invoke("list_projects", { root: state.root });
    await openProject(p);
    log(`已创建工程 ${name}`);
  } catch (e) { log("创建失败: " + e, "err"); }
}

/* ---------- 打包 / 运行 ---------- */
async function pack() {
  if (!state.current) return log("请先新建/选择工程再打包", "warn");
  try {
    if (state.activeTab) await saveFile();
    const res = await invoke("pack_kir", {
      projectRoot: state.current,
      codePath: "src/main.rs",
      iconPath: null,
    });
    log(`打包完成：${res.path}（${(res.size / 1024).toFixed(1)} KB）`);
    await refreshFiles();
  } catch (e) { log("打包失败: " + e, "err"); }
}

async function runKernel() {
  try {
    await invoke("run_qemu", { kernel: "build/qemu-kernel.elf", appsDir: null });
    log("QEMU 已启动（串口输出在终端）");
  } catch (e) { log("运行失败: " + e, "err"); }
}

async function refreshFiles() {
  if (!state.current) return;
  try {
    state.files = await invoke("list_files", { projectRoot: state.current });
    renderFileTree();
  } catch (e) { log("刷新文件树失败: " + e, "err"); }
}

/* ---------- Glass 性能档 ---------- */
function setGlassMode(mode) {
  document.body.dataset.glass = mode;
  document.querySelectorAll(".mode-btn").forEach(b => b.classList.toggle("active", b.dataset.mode === mode));
  document.getElementById("sb-glass").textContent =
    "Glass: " + (mode === "ultra" ? "极致" : mode === "balance" ? "平衡" : "性能");
  log(`Glass 渲染档位：${mode === "ultra" ? "极致" : mode === "balance" ? "平衡" : "性能"}`);
}

/* ---------- 底部工具窗口折叠 ---------- */
function toggleCollapse() {
  const tw = document.getElementById("tool-window");
  const collapsed = tw.classList.toggle("collapsed");
  document.getElementById("btn-collapse").textContent = collapsed ? "▷" : "▽";
}

/* ---------- Mock（浏览器预览） ---------- */
const mockStore = { projects: [], files: {} };
function mockInvoke(cmd, args) {
  switch (cmd) {
    case "list_projects": {
      if (mockStore.projects.length === 0) {
        mockStore.projects = [
          { name: "hello", path: "/demo/hello", hasManifest: true, modified: 0 },
          { name: "calculator", path: "/demo/calculator", hasManifest: true, modified: 0 },
        ];
      }
      return Promise.resolve(mockStore.projects);
    }
    case "new_project": {
      const p = { name: args.name, path: "/demo/" + args.name, hasManifest: true, modified: 0 };
      mockStore.projects.push(p);
      mockStore.files["src/main.rs"] = "// Space OS 应用\n";
      mockStore.files["manifest.json"] = '{"name":"' + args.name + '"}';
      return Promise.resolve(p);
    }
    case "list_files":
      return Promise.resolve(Object.keys(mockStore.files).concat("src/"));
    case "read_file":
      return Promise.resolve(mockStore.files[args.rel] ?? "// Space OS 应用\n");
    case "write_file":
      mockStore.files[args.rel] = args.content;
      return Promise.resolve(null);
    case "pack_kir":
      return Promise.resolve({ path: "/demo/build/app.kir", size: 1234 });
    case "run_qemu":
      return Promise.resolve(null);
    default:
      return Promise.reject("未知命令（浏览器预览模式）");
  }
}

function escapeHtml(s) {
  return s.replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}

/* ---------- 初始化 ---------- */
function init() {
  drawWallpaper();

  const area = document.getElementById("code-area");
  area.addEventListener("input", () => {
    updateLineNums();
    const pos = area.selectionStart;
    const line = area.value.slice(0, pos).split("\n").length;
    const col = pos - area.value.slice(0, pos).lastIndexOf("\n");
    document.getElementById("sb-cursor").textContent = `Ln ${line}, Col ${col}`;
  });
  area.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
      e.preventDefault();
      saveFile();
    }
  });
  area.addEventListener("click", () => {
    const pos = area.selectionStart;
    const line = area.value.slice(0, pos).split("\n").length;
    const col = pos - area.value.slice(0, pos).lastIndexOf("\n");
    document.getElementById("sb-cursor").textContent = `Ln ${line}, Col ${col}`;
  });

  document.getElementById("btn-new-project").addEventListener("click", showModal);
  document.getElementById("btn-refresh").addEventListener("click", refreshFiles);
  document.getElementById("modal-cancel").addEventListener("click", hideModal);
  document.getElementById("modal-ok").addEventListener("click", createProject);
  document.getElementById("modal-input").addEventListener("keydown", (e) => {
    if (e.key === "Enter") createProject();
    if (e.key === "Escape") hideModal();
  });
  document.getElementById("btn-pack").addEventListener("click", pack);
  document.getElementById("btn-run").addEventListener("click", runKernel);
  document.getElementById("btn-collapse").addEventListener("click", toggleCollapse);
  document.querySelectorAll(".mode-btn").forEach(b => b.addEventListener("click", () => setGlassMode(b.dataset.mode)));
  document.querySelectorAll(".side-tab").forEach(b => b.addEventListener("click", () => {
    document.querySelectorAll(".side-tab").forEach(x => x.classList.toggle("active", x === b));
    log(`侧栏视图：${b.textContent}`);
  }));

  if (!isTauri) {
    document.getElementById("sb-mode").textContent = "预览";
    log("浏览器预览模式（无 Tauri 后端，工程数据为本地 mock）");
  } else {
    document.getElementById("sb-mode").textContent = "Tauri";
    log("Space Studio 后端已连接");
  }

  refreshProjects();
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", init);
} else {
  init();
}
