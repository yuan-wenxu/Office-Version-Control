const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const FALLBACK_REFRESH_MS = 5000;
const WATCH_REFRESH_DEBOUNCE_MS = 250;
const WATCH_SETTLED_REFRESH_MS = 600;

const state = {
  workspace: "",
  historyFile: "",
  history: [],
  trackedFiles: [],
  diffMode: "versions",
  selectionRequest: 0,
  diffRequest: 0,
  latestDiff: null,
  activeDiffTab: "text",
  autoRefreshTimer: null,
  watchRefreshTimer: null,
  watchSettledRefreshTimer: null,
  watchUnlisten: null,
  statusRefreshRunning: false,
  statusRefreshPending: false,
  stagedCount: 0,
  visibleChanges: new Map(),
  stagedFingerprint: "",
  generatingMessage: false,
  repositoryBusy: false
};

const elements = {
  workspacePath: document.querySelector("#workspacePath"),
  messageLine: document.querySelector("#messageLine"),
  workspaceMenu: document.querySelector("#workspaceMenu"),
  stagedOutput: document.querySelector("#stagedOutput"),
  statusOutput: document.querySelector("#statusOutput"),
  commitMessage: document.querySelector("#commitMessage"),
  commitBtn: document.querySelector("#commitBtn"),
  generateMessageBtn: document.querySelector("#generateMessageBtn"),
  aiSettingsDialog: document.querySelector("#aiSettingsDialog"),
  aiSettingsForm: document.querySelector("#aiSettingsForm"),
  aiProvider: document.querySelector("#aiProvider"),
  aiBaseUrl: document.querySelector("#aiBaseUrl"),
  aiModel: document.querySelector("#aiModel"),
  aiApiKey: document.querySelector("#aiApiKey"),
  aiSupportsImages: document.querySelector("#aiSupportsImages"),
  aiSettingsPath: document.querySelector("#aiSettingsPath"),
  aiSettingsError: document.querySelector("#aiSettingsError"),
  saveAiSettingsBtn: document.querySelector("#saveAiSettingsBtn"),
  historyList: document.querySelector("#historyList"),
  trackedFilesList: document.querySelector("#trackedFilesList"),
  diffHeading: document.querySelector("#diffHeading"),
  diffControls: document.querySelector("#diffControls"),
  diffBtn: document.querySelector("#diffBtn"),
  diffFileLabel: document.querySelector("#diffFileLabel"),
  fromVersionSelect: document.querySelector("#fromVersionSelect"),
  toVersionSelect: document.querySelector("#toVersionSelect"),
  diffOutput: document.querySelector("#diffOutput"),
  diffTextTab: document.querySelector("#diffTextTab"),
  diffImagesTab: document.querySelector("#diffImagesTab")
};

updateUiScale();
window.addEventListener("resize", updateUiScale, { passive: true });
setupWorkspaceChangeListener();
setupWindowFocusListener();
updateCommitButtonState();

elements.commitMessage.addEventListener("input", updateCommitButtonState);

elements.workspaceMenu.addEventListener("click", (event) => {
  if (event.target.closest("button")) elements.workspaceMenu.open = false;
});

document.addEventListener("click", (event) => {
  if (!elements.workspaceMenu.contains(event.target)) elements.workspaceMenu.open = false;
});

elements.workspaceMenu.addEventListener("keydown", (event) => {
  if (event.key !== "Escape" || !elements.workspaceMenu.open) return;
  elements.workspaceMenu.open = false;
  elements.workspaceMenu.querySelector("summary").focus();
});

const apiDefaults = {
  openai: "https://api.openai.com/v1",
  anthropic: "https://api.anthropic.com/v1"
};

document.querySelector("#aiSettingsBtn").addEventListener("click", () => run(async () => {
  showAiSettings(await invoke("load_ai_settings"));
}));

document.querySelector("#cancelAiSettingsBtn").addEventListener("click", () => {
  elements.aiSettingsDialog.close();
});

elements.aiSettingsDialog.addEventListener("close", () => {
  elements.aiApiKey.value = "";
});

elements.aiProvider.addEventListener("change", () => {
  if (!elements.aiBaseUrl.value || Object.values(apiDefaults).includes(elements.aiBaseUrl.value)) {
    elements.aiBaseUrl.value = apiDefaults[elements.aiProvider.value];
  }
});

elements.aiSettingsForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (elements.saveAiSettingsBtn.disabled) return;
  elements.saveAiSettingsBtn.disabled = true;
  elements.aiSettingsError.hidden = true;
  try {
    await invoke("save_ai_settings", {
      settings: {
        provider: elements.aiProvider.value,
        baseUrl: elements.aiBaseUrl.value.trim(),
        model: elements.aiModel.value.trim(),
        supportsImages: elements.aiSupportsImages.checked
      },
      apiKey: elements.aiApiKey.value.trim()
    });
    elements.aiSettingsDialog.close();
    setMessage("AI API settings saved.");
  } catch (error) {
    elements.aiSettingsError.textContent = error?.message || String(error);
    elements.aiSettingsError.hidden = false;
  } finally {
    elements.saveAiSettingsBtn.disabled = false;
  }
});

elements.generateMessageBtn.addEventListener("click", () => run(generateCommitMessage));

function showAiSettings({ settings, hasKey, path }) {
  elements.aiProvider.value = settings.provider;
  elements.aiBaseUrl.value = settings.baseUrl || apiDefaults[settings.provider];
  elements.aiModel.value = settings.model;
  elements.aiSupportsImages.checked = settings.supportsImages;
  elements.aiApiKey.value = "";
  elements.aiApiKey.placeholder = hasKey ? "Leave blank to keep the saved key" : "Enter API key";
  elements.aiSettingsPath.textContent = path;
  elements.aiSettingsError.hidden = true;
  if (!elements.aiSettingsDialog.open) elements.aiSettingsDialog.showModal();
}

async function generateCommitMessage() {
  if (elements.generateMessageBtn.disabled) return;
  const workspace = state.workspace;
  const staged = state.stagedFingerprint;
  const draft = elements.commitMessage.value;
  state.generatingMessage = true;
  updateCommitButtonState();
  try {
    const settings = await invoke("load_ai_settings");
    if (state.workspace !== workspace) return;
    if (!settings.hasKey) {
      showAiSettings(settings);
      return;
    }

    const message = await invoke("generate_commit_message", { workspace });
    if (state.workspace !== workspace || state.stagedFingerprint !== staged) {
      setMessage("The staging area changed. Generate a new message for the current files.");
      return;
    }
    if (elements.commitMessage.value !== draft) {
      setMessage("Commit message changed while AI was generating.");
      return;
    }
    elements.commitMessage.value = message;
    setMessage("Commit message generated.");
  } finally {
    state.generatingMessage = false;
    updateCommitButtonState();
  }
}

document.querySelector("#selectWorkspaceBtn").addEventListener("click", async () => {
  await run(async () => {
    const workspace = await invoke("select_workspace");
    if (!workspace) return;
    state.workspace = workspace;
    state.historyFile = "";
    state.history = [];
    state.trackedFiles = [];
    state.diffMode = "versions";
    state.selectionRequest += 1;
    state.latestDiff = null;
    state.stagedCount = 0;
    state.visibleChanges.clear();
    state.stagedFingerprint = "";
    updateCommitButtonState();
    elements.workspacePath.textContent = workspace;
    elements.diffFileLabel.textContent = "Choose a tracked file.";
    renderTrackedFiles();
    renderHistory();
    renderStatus({});
    renderDiffControls();
    localStorage.setItem("ovc.workspace", workspace);
    await startWorkspaceWatch();
    await refreshStatus();
    await refreshTrackedFiles();
    startFallbackRefresh();
    setMessage("Workspace selected.");
  });
});

document.querySelector("#initBtn").addEventListener("click", async () => {
  await run(async () => {
    requireWorkspace();
    await invoke("init_repo", { workspace: state.workspace });
    await refreshStatus();
    await refreshTrackedFiles();
    setMessage("Repository initialized.");
  });
});

document.querySelector("#selectFilesBtn").addEventListener("click", async () => {
  await run(async () => {
    requireWorkspace();
    const workspace = state.workspace;
    const files = await invoke("select_office_files");
    if (!files.length || state.workspace !== workspace) return;
    await mutateRepository(async () => {
      const staged = await invoke("track_files", { workspace, files });
      if (state.workspace !== workspace) return;
      await refreshStatus();
      await refreshTrackedFiles();
      setMessage(`Added and staged ${staged.length} file(s).`);
    });
  });
});

document.querySelector("#statusBtn").addEventListener("click", () => run(refreshStatus));
document.querySelector("#logBtn").addEventListener("click", async () => {
  await run(async () => {
    await refreshTrackedFiles();
    if (state.historyFile) {
      await selectTrackedFile(state.historyFile, false, state.diffMode);
    }
    setMessage("History refreshed.");
  });
});

elements.diffTextTab.addEventListener("click", () => {
  state.activeDiffTab = "text";
  setDiffTabState();
  if (state.latestDiff) renderDiff(state.latestDiff);
});

elements.diffImagesTab.addEventListener("click", () => {
  state.activeDiffTab = "images";
  setDiffTabState();
  if (state.latestDiff) renderDiff(state.latestDiff);
});

document.querySelector("#commitBtn").addEventListener("click", async () => {
  await run(async () => {
    if (elements.commitBtn.disabled) return;
    requireWorkspace();
    const message = elements.commitMessage.value.trim();
    if (!message) throw new Error("Commit message is required.");
    const workspace = state.workspace;
    await mutateRepository(async () => {
      const records = await invoke("commit_staged", { workspace, message });
      if (state.workspace !== workspace) return;
      elements.commitMessage.value = "";
      await refreshStatus();
      await refreshTrackedFiles();
      if (state.historyFile) await selectTrackedFile(state.historyFile, false);
      setMessage(records.length ? `Committed ${records.length} file(s).` : "The staged files already match their latest versions.");
    });
  });
});

elements.diffBtn.addEventListener("click", async () => {
  await run(async () => {
    requireWorkspace();
    const filePath = state.historyFile;
    if (!filePath) throw new Error("Choose a tracked file before comparing versions.");
    if (state.diffMode !== "versions") {
      await selectTrackedFile(filePath, false, state.diffMode);
      return;
    }
    const fromVersion = elements.fromVersionSelect.value;
    const toVersion = elements.toVersionSelect.value;
    if (!fromVersion || !toVersion) throw new Error("Choose two versions before diffing.");
    if (fromVersion === toVersion) throw new Error("Choose two different versions.");
    const request = ++state.diffRequest;
    const selection = state.selectionRequest;
    const diff = await invoke("diff_versions", {
      workspace: state.workspace,
      filePath,
      fromVersion,
      toVersion
    });
    if (request !== state.diffRequest || selection !== state.selectionRequest) return;
    state.latestDiff = diff;
    renderDiff(diff);
    setMessage("Diff loaded.");
  });
});

window.addEventListener("DOMContentLoaded", async () => {
  const workspace = localStorage.getItem("ovc.workspace");
  if (!workspace) return;
  state.workspace = workspace;
  elements.workspacePath.textContent = workspace;
  await run(async () => {
    await startWorkspaceWatch();
    await refreshStatus();
    await refreshTrackedFiles();
    renderHistory();
    renderDiffVersionOptions();
    startFallbackRefresh();
  });
});

document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible") {
    run(refreshStatus);
  }
});

async function refreshStatus() {
  requireWorkspace();
  if (state.statusRefreshRunning) {
    // Another refresh is in flight — mark as pending so it reruns after completion
    state.statusRefreshPending = true;
    return;
  }
  state.statusRefreshRunning = true;
  state.statusRefreshPending = false;
  try {
    const workspace = state.workspace;
    const status = await invoke("status", { workspace });
    if (state.workspace !== workspace) return;
    const modified = await filterVisibleChanges(workspace, status);
    if (state.workspace !== workspace) return;
    const staged = status.staged || [];
    state.stagedCount = staged.length;
    state.stagedFingerprint = JSON.stringify(staged.map((file) => [file.sourcePath, file.hash]).sort());
    renderStatus({ ...status, modified });
    updateCommitButtonState();
  } finally {
    state.statusRefreshRunning = false;
    if (state.statusRefreshPending) {
      state.statusRefreshPending = false;
      window.setTimeout(() => run(refreshStatus), 0);
    }
  }
}

async function filterVisibleChanges(workspace, status) {
  const staged = new Map((status.staged || []).map((file) => [file.sourcePath, file]));
  const cache = new Map();
  const visible = [];
  for (const file of status.modified || []) {
    if (state.workspace !== workspace) break;
    const snapshot = staged.get(file.sourcePath);
    if (snapshot?.hash === file.currentHash) continue;
    const baseHash = snapshot?.snapshot ? snapshot.hash : file.previousHash;
    if (!baseHash) {
      visible.push(file);
      continue;
    }
    const key = JSON.stringify([file.sourcePath, baseHash, file.currentHash]);
    let changed = state.visibleChanges.get(key);
    if (changed === undefined) {
      try {
        changed = await invoke("has_visible_change", {
          workspace, filePath: file.sourcePath, baseHash, currentHash: file.currentHash
        });
      } catch {
        // Keep files visible if Office is still saving or the package cannot be read.
        visible.push(file);
        continue;
      }
    }
    if (changed === null) continue;
    cache.set(key, changed);
    if (changed) visible.push(file);
  }
  if (state.workspace === workspace) state.visibleChanges = cache;
  return visible;
}

function renderStatus(status) {
  const staged = new Map((status.staged || []).map((item) => [item.sourcePath, item]));
  renderChangeList(elements.statusOutput, status.modified || [], "working", "No unstaged changes.");
  renderChangeList(elements.stagedOutput, [...staged.values()], "staged", "No staged files.");
}

function renderChangeList(container, files, mode, emptyMessage) {
  container.textContent = "";
  if (!files.length) {
    const empty = document.createElement("div");
    empty.className = "status-empty";
    empty.textContent = emptyMessage;
    container.append(empty);
    return;
  }

  const staged = mode === "staged";
  for (const item of [...files].sort((a, b) => a.sourcePath.localeCompare(b.sourcePath))) {
    const row = document.createElement("div");
    row.className = "change-row";

    const preview = document.createElement("button");
    preview.className = "status-item";
    preview.type = "button";
    preview.title = item.sourcePath;
    preview.addEventListener("click", () => run(() => selectTrackedFile(item.sourcePath, true, mode)));

    const title = document.createElement("strong");
    title.textContent = displayPath(item.sourcePath);
    const meta = document.createElement("small");
    meta.textContent = staged ? shortHash(item.hash) : shortHash(item.currentHash);
    preview.append(title, meta);

    const action = document.createElement("button");
    action.className = "stage-action";
    action.type = "button";
    action.textContent = staged ? "Unstage" : "Stage";
    action.setAttribute("aria-label", `${action.textContent} ${item.sourcePath}`);
    action.addEventListener("click", () => run(() => updateStage(item.sourcePath, !staged)));

    row.append(preview, action);
    container.append(row);
  }
}

async function mutateRepository(task) {
  if (state.repositoryBusy) return;
  state.repositoryBusy = true;
  updateCommitButtonState();
  try {
    await task();
  } finally {
    state.repositoryBusy = false;
    updateCommitButtonState();
  }
}

async function updateStage(filePath, stage) {
  await mutateRepository(async () => {
    const workspace = state.workspace;
    const selection = state.selectionRequest;
    await invoke(stage ? "stage_file" : "unstage_file", { workspace, filePath });
    if (state.workspace !== workspace) return;
    await refreshStatus();
    await refreshTrackedFiles();
    if (selection === state.selectionRequest && state.historyFile === filePath) {
      await selectTrackedFile(filePath, false, stage ? "staged" : "working");
    }
    setMessage(`${stage ? "Staged" : "Unstaged"} ${displayPath(filePath)}.`);
  });
}

async function refreshTrackedFiles() {
  requireWorkspace();
  const workspace = state.workspace;
  const trackedFiles = await invoke("tracked_files", { workspace });
  if (state.workspace !== workspace) return;
  state.trackedFiles = trackedFiles;
  renderTrackedFiles();
  if (state.historyFile && !state.trackedFiles.some((file) => file.sourcePath === state.historyFile)) {
    state.historyFile = "";
    state.history = [];
    state.diffMode = "versions";
    state.selectionRequest += 1;
    state.latestDiff = null;
    elements.diffFileLabel.textContent = "Choose a tracked file.";
    renderHistory();
    renderDiffControls();
  }
}

async function selectTrackedFile(filePath, announce = true, mode = "versions") {
  requireWorkspace();
  const workspace = state.workspace;
  const selection = ++state.selectionRequest;
  const label = displayPath(filePath);
  state.historyFile = filePath;
  state.diffMode = mode;
  state.history = [];
  state.latestDiff = null;
  elements.diffFileLabel.textContent = label;
  renderTrackedFiles();
  elements.historyList.textContent = "Loading history…";
  renderDiffControls();

  try {
    const records = await invoke("log", { workspace, filePath });
    if (selection !== state.selectionRequest) return;
    state.history = records;
    renderHistory();
    if (mode !== "versions") {
      const command = mode === "staged" ? "diff_staged" : "diff_working";
      const diff = await invoke(command, { workspace, filePath });
      if (selection !== state.selectionRequest) return;
      state.latestDiff = diff;
      renderDiff(diff);
    } else {
      renderDiffControls();
    }
    if (announce) setMessage(`Loaded ${label}.`);
  } catch (error) {
    if (selection !== state.selectionRequest) return;
    renderHistory();
    renderDiffMessage("Could not load the comparison.");
    throw error;
  }
}

function renderDiffControls() {
  const comparingVersions = state.diffMode === "versions";
  const headings = { working: "Working copy diff", staged: "Staged diff", versions: "Version diff" };
  elements.diffHeading.textContent = headings[state.diffMode];
  elements.diffControls.hidden = !comparingVersions;
  elements.diffBtn.textContent = comparingVersions ? "Compare" : "Refresh diff";
  if (comparingVersions) {
    renderDiffVersionOptions();
  } else {
    renderDiffMessage("Loading comparison…");
  }
}

function renderTrackedFiles() {
  if (!state.trackedFiles.length) {
    elements.trackedFilesList.textContent = "No tracked files.";
    return;
  }

  elements.trackedFilesList.textContent = "";
  for (const file of state.trackedFiles) {
    const button = document.createElement("button");
    button.className = "tracked-file";
    button.type = "button";
    button.dataset.selected = String(file.sourcePath === state.historyFile);
    button.addEventListener("click", () => run(() => selectTrackedFile(file.sourcePath)));

    const name = document.createElement("strong");
    name.textContent = file.originalName;

    const meta = document.createElement("small");
    const versionText = file.versions === 1 ? "1 version" : `${file.versions} versions`;
    const stagedText = file.staged ? " · staged" : "";
    meta.textContent = `${file.sourcePath} · ${versionText}${stagedText}`;

    button.append(name, meta);
    elements.trackedFilesList.append(button);
  }
}

function renderHistory() {
  if (!state.history.length) {
    elements.historyList.textContent = state.historyFile
      ? "No committed versions for this file yet."
      : "Choose a tracked file to view versions.";
    return;
  }

  elements.historyList.textContent = "";
  for (const record of state.history) {
    const item = document.createElement("div");
    item.className = "history-item";

    const id = document.createElement("strong");
    id.textContent = record.id;

    const meta = document.createElement("div");
    meta.className = "history-meta";
    meta.innerHTML = `<div>${escapeHtml(record.originalName)} - ${escapeHtml(record.message || "No message")}</div>
      <small>${escapeHtml(record.createdAt)} · ${escapeHtml(record.hash.slice(0, 12))}</small>`;

    const checkout = document.createElement("button");
    checkout.textContent = "Checkout";
    checkout.addEventListener("click", () => {
      run(async () => {
        const output = await invoke("select_output_file", { defaultPath: record.sourcePath });
        if (!output) return;
        const restored = await invoke("checkout_version", {
          workspace: state.workspace,
          filePath: record.sourcePath,
          version: record.id,
          output
        });
        setMessage(`Restored ${restored}`);
      });
    });

    item.append(id, meta, checkout);
    elements.historyList.append(item);
  }
}

function renderDiffVersionOptions() {
  elements.fromVersionSelect.textContent = "";
  elements.toVersionSelect.textContent = "";

  if (!state.history.length) {
    elements.fromVersionSelect.append(new Option("From", ""));
    elements.toVersionSelect.append(new Option("To", ""));
    renderDiffMessage(state.historyFile
      ? "At least two versions are required to diff this file."
      : "Choose a change or a tracked file.");
    return;
  }

  const records = [...state.history].sort((a, b) => versionNumber(a.id) - versionNumber(b.id));
  for (const record of records) {
    const label = `${record.id} - ${record.message || "No message"}`;
    elements.fromVersionSelect.append(new Option(label, record.id));
    elements.toVersionSelect.append(new Option(label, record.id));
  }

  if (records.length >= 2) {
    elements.fromVersionSelect.value = records[records.length - 2].id;
    elements.toVersionSelect.value = records[records.length - 1].id;
  }
  renderDiffMessage(records.length < 2
    ? "At least two versions are required to diff this file."
    : "Choose versions, then click Compare.");
}

function renderDiff(diff) {
  if (state.activeDiffTab === "images") {
    renderImageDiff(diff);
  } else {
    renderTextDiff(diff);
  }
}

function renderTextDiff(diff) {
  elements.diffOutput.textContent = "";
  elements.diffOutput.dataset.view = "text";

  const summary = document.createElement("div");
  summary.className = "diff-summary";

  const version = document.createElement("div");
  version.className = "diff-version";
  const title = document.createElement("strong");
  title.textContent = `${diff.from.id} -> ${diff.to.id}`;
  const meta = document.createElement("small");
  meta.textContent = `${diff.from.message || "No message"} -> ${diff.to.message || "No message"}`;
  version.append(title, meta);

  const added = document.createElement("div");
  added.className = "diff-stat";
  added.dataset.kind = "added";
  added.append(statStrong(`+${diff.addedLines}`), statSmall("added"));

  const removed = document.createElement("div");
  removed.className = "diff-stat";
  removed.dataset.kind = "removed";
  removed.append(statStrong(`-${diff.removedLines}`), statSmall("removed"));

  summary.append(version, added, removed);

  const list = document.createElement("div");
  list.className = "diff-list";

  if (!diff.lines.length || (!diff.addedLines && !diff.removedLines)) {
    const empty = document.createElement("div");
    empty.className = "diff-empty";
    empty.textContent = diff.packageChanges?.length
      ? "Text and images match. These Office package parts changed:"
      : "No text changes detected between these versions.";
    if (diff.packageChanges?.length) {
      const parts = document.createElement("ul");
      for (const change of diff.packageChanges) {
        const part = document.createElement("li");
        part.textContent = change;
        parts.append(part);
      }
      empty.append(parts);
    }
    list.append(empty);
  } else {
    for (const line of diff.lines) {
      list.append(renderDiffLine(line));
    }
  }

  elements.diffOutput.append(summary, list);
}

function renderImageDiff(diff) {
  elements.diffOutput.textContent = "";
  elements.diffOutput.dataset.view = "images";

  const summary = document.createElement("div");
  summary.className = "diff-summary diff-summary-images";

  const version = document.createElement("div");
  version.className = "diff-version";
  version.append(
    statStrong(`${diff.from.id} -> ${diff.to.id}`),
    statSmall(`${diff.from.message || "No message"} -> ${diff.to.message || "No message"}`)
  );

  const added = imageStat("+", diff.addedImages, "added");
  const removed = imageStat("-", diff.removedImages, "removed");
  const modified = imageStat("~", diff.modifiedImages, "changed");
  summary.append(version, added, removed, modified);

  const card = document.createElement("div");
  card.className = "image-diff-card";

  const changedImages = (diff.images || []).filter((image) => image.kind !== "unchanged");
  if (!changedImages.length) {
    const empty = document.createElement("div");
    empty.className = "diff-empty";
    empty.textContent = diff.unchangedImages
      ? `No image changes detected. ${diff.unchangedImages} image(s) are unchanged.`
      : "No images detected between these versions.";
    card.append(empty);
  } else {
    for (const image of changedImages) {
      card.append(renderImageDiffItem(image));
    }
  }

  elements.diffOutput.append(summary, card);
}

function renderImageDiffItem(image) {
  const item = document.createElement("article");
  item.className = "image-diff-item";
  item.dataset.kind = image.kind;

  const header = document.createElement("header");
  const title = document.createElement("strong");
  title.textContent = displayMediaName(image.path);
  const badge = document.createElement("span");
  badge.className = "image-diff-badge";
  badge.textContent = image.kind;
  header.append(title, badge);

  const previews = document.createElement("div");
  previews.className = "image-preview-pair";

  if (image.kind !== "added") {
    previews.append(renderImagePreview("Before", image.oldDataUrl, image.oldSize, image.oldHash));
  }

  if (image.kind !== "removed") {
    previews.append(renderImagePreview("After", image.newDataUrl, image.newSize, image.newHash));
  }

  item.append(header, previews);
  return item;
}

function renderImagePreview(label, dataUrl, size, hash) {
  const preview = document.createElement("div");
  preview.className = "image-preview";

  const imageWrap = document.createElement("div");
  imageWrap.className = "image-preview-frame";
  if (dataUrl) {
    const image = document.createElement("img");
    image.src = dataUrl;
    image.alt = label;
    image.loading = "lazy";
    image.addEventListener("load", () => updateImagePairLayout(image));
    imageWrap.append(image);
  } else {
    const unsupported = document.createElement("span");
    unsupported.textContent = "Preview not supported";
    imageWrap.append(unsupported);
  }

  const meta = document.createElement("div");
  meta.className = "image-preview-meta";
  meta.append(statStrong(label), statSmall(`${formatBytes(size)} · ${shortHash(hash)}`));

  preview.append(imageWrap, meta);
  return preview;
}

function renderDiffLine(line) {
  const row = document.createElement("div");
  row.className = "diff-row";
  row.dataset.kind = line.kind;

  const oldLine = document.createElement("span");
  oldLine.className = "diff-line-number";
  oldLine.textContent = line.oldLine ?? "";

  const newLine = document.createElement("span");
  newLine.className = "diff-line-number";
  newLine.textContent = line.newLine ?? "";

  const marker = document.createElement("span");
  marker.className = "diff-marker";
  marker.textContent = line.kind === "added" ? "+" : line.kind === "removed" ? "-" : "";

  const text = document.createElement("span");
  text.className = "diff-text";
  text.textContent = line.text || " ";

  row.append(oldLine, newLine, marker, text);
  return row;
}

function renderDiffMessage(message) {
  elements.diffOutput.textContent = "";
  const empty = document.createElement("div");
  empty.className = "diff-empty";
  empty.textContent = message;
  elements.diffOutput.append(empty);
}

function statStrong(value) {
  const strong = document.createElement("strong");
  strong.textContent = value;
  return strong;
}

function statSmall(value) {
  const small = document.createElement("small");
  small.textContent = value;
  return small;
}

function imageStat(prefix, count, label) {
  const item = document.createElement("div");
  item.className = "diff-stat";
  item.dataset.kind = label === "removed" ? "removed" : label === "added" ? "added" : "modified";
  item.append(statStrong(`${prefix}${count}`), statSmall(label));
  return item;
}

function setDiffTabState() {
  elements.diffTextTab.dataset.active = String(state.activeDiffTab === "text");
  elements.diffImagesTab.dataset.active = String(state.activeDiffTab === "images");
}

function updateImagePairLayout(image) {
  const pair = image.closest(".image-preview-pair");
  if (!pair) return;
  if (image.naturalHeight > image.naturalWidth) {
    pair.dataset.layout = "stacked";
    return;
  }

  const images = [...pair.querySelectorAll("img")];
  const hasPortrait = images.some((item) => item.complete && item.naturalHeight > item.naturalWidth);
  pair.dataset.layout = hasPortrait ? "stacked" : "side-by-side";
}

function formatBytes(value) {
  if (!Number.isFinite(value)) return "unknown size";
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
  return `${(value / 1024 / 1024).toFixed(1)} MB`;
}

function shortHash(value) {
  return value ? value.slice(0, 12) : "no hash";
}

function displayMediaName(path) {
  return path.split(/[\\/]/).pop() || path;
}

async function run(task) {
  try {
    await task();
  } catch (error) {
    setMessage(error?.message || String(error), true);
  }
}

async function setupWorkspaceChangeListener() {
  if (state.watchUnlisten) return;
  // Only one workspace is watched at a time, so any incoming event
  // is always for the current workspace - no path comparison needed.
  state.watchUnlisten = await listen("workspace-files-changed", () => {
    if (!state.workspace) return;
    scheduleWatchRefresh();
  });
}

function setupWindowFocusListener() {
  // tauri://focus is more reliable than visibilitychange inside a Tauri webview.
  // When the user switches back to OVC after editing a file, this triggers an
  // immediate status refresh instead of waiting for the fallback timer.
  listen("tauri://focus", () => {
    if (state.workspace) run(refreshStatus);
  });
}

async function startWorkspaceWatch() {
  if (!state.workspace) return;
  await invoke("watch_workspace", { workspace: state.workspace });
}

function scheduleWatchRefresh() {
  if (state.watchRefreshTimer) {
    window.clearTimeout(state.watchRefreshTimer);
  }
  if (state.watchSettledRefreshTimer) {
    window.clearTimeout(state.watchSettledRefreshTimer);
  }

  state.watchRefreshTimer = window.setTimeout(() => {
    run(refreshStatus);
  }, WATCH_REFRESH_DEBOUNCE_MS);

  state.watchSettledRefreshTimer = window.setTimeout(() => {
    run(async () => {
      await refreshStatus();
      setMessage("File changes checked.");
    });
  }, WATCH_SETTLED_REFRESH_MS);
}

function startFallbackRefresh() {
  if (state.autoRefreshTimer) {
    window.clearInterval(state.autoRefreshTimer);
  }
  state.autoRefreshTimer = window.setInterval(() => {
    if (!state.workspace || document.visibilityState !== "visible") return;
    run(refreshStatus);
  }, FALLBACK_REFRESH_MS);
}

function updateCommitButtonState() {
  const hasMessage = elements.commitMessage.value.trim().length > 0;
  const canCommit = Boolean(state.workspace) && state.stagedCount > 0 && hasMessage && !state.repositoryBusy;
  elements.commitBtn.disabled = !canCommit;
  elements.generateMessageBtn.disabled = !state.workspace || state.stagedCount === 0
    || state.repositoryBusy || state.generatingMessage;
  elements.generateMessageBtn.textContent = state.generatingMessage ? "Generating…" : "AI draft";
  elements.commitBtn.title = canCommit
    ? "Commit staged files"
    : "Stage files and enter a commit message.";
}

function requireWorkspace() {
  if (!state.workspace) throw new Error("Choose a workspace first.");
}

function setMessage(message, isError = false) {
  elements.messageLine.textContent = message;
  elements.messageLine.dataset.error = String(isError);
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");
}

function versionNumber(id) {
  const match = /^v(\d+)$/.exec(id);
  return match ? Number(match[1]) : Number.MAX_SAFE_INTEGER;
}

function displayPath(filePath) {
  if (!state.workspace) return filePath;
  const normalizedWorkspace = state.workspace.replaceAll("\\", "/").replace(/\/+$/, "").toLowerCase();
  const normalizedFile = filePath.replaceAll("\\", "/");
  if (normalizedFile.toLowerCase().startsWith(`${normalizedWorkspace}/`)) {
    return normalizedFile.slice(normalizedWorkspace.length + 1);
  }
  return filePath;
}

function samePath(left, right) {
  if (!left || !right) return false;
  return left.replaceAll("\\", "/").replace(/\/+$/, "").toLowerCase()
    === right.replaceAll("\\", "/").replace(/\/+$/, "").toLowerCase();
}

function updateUiScale() {
  const widthScale = window.innerWidth / 1280;
  const heightScale = window.innerHeight / 820;
  const scale = Math.min(1.5, Math.max(0.92, Math.min(widthScale, heightScale * 1.15)));
  document.documentElement.style.setProperty("--ui-scale", scale.toFixed(3));
}
