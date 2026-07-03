const vscode = require("vscode");
const { LanguageClient } = require("vscode-languageclient/node");
const cp = require("child_process");
const fs = require("fs");
const https = require("https");
const os = require("os");
const path = require("path");

let client;

// ---------------------------------------------------------------------------
// Binary resolution
// ---------------------------------------------------------------------------

const REPO = "Almaju/canon";

function exeName() {
  return process.platform === "win32" ? "canon.exe" : "canon";
}

function releaseTarget() {
  const arch = process.arch; // 'x64' | 'arm64' | ...
  switch (process.platform) {
    case "darwin":
      return arch === "arm64" ? "aarch64-apple-darwin" : "x86_64-apple-darwin";
    case "linux":
      return arch === "arm64"
        ? "aarch64-unknown-linux-gnu"
        : "x86_64-unknown-linux-gnu";
    case "win32":
      return "x86_64-pc-windows-msvc";
    default:
      return null;
  }
}

function isExecutable(file) {
  try {
    fs.accessSync(file, fs.constants.X_OK);
    return fs.statSync(file).isFile();
  } catch {
    return false;
  }
}

function findOnPath() {
  const dirs = (process.env.PATH || "").split(path.delimiter);
  for (const dir of dirs) {
    if (!dir) continue;
    const candidate = path.join(dir, exeName());
    if (isExecutable(candidate)) return candidate;
  }
  return null;
}

function findDownloaded(storageDir) {
  try {
    const entries = fs
      .readdirSync(storageDir)
      .filter((e) => e.startsWith("canon-"))
      .sort()
      .reverse(); // newest version first (lexicographic is good enough per major)
    for (const entry of entries) {
      const nested = path.join(storageDir, entry, entry, exeName());
      if (isExecutable(nested)) return nested;
      const flat = path.join(storageDir, entry, exeName());
      if (isExecutable(flat)) return flat;
    }
  } catch {
    /* storage dir doesn't exist yet */
  }
  return null;
}

function resolveBinary(context) {
  const configured = vscode.workspace
    .getConfiguration("canon")
    .get("serverPath");
  if (configured) {
    if (isExecutable(configured)) return configured;
    vscode.window.showWarningMessage(
      `canon.serverPath is set to "${configured}" but it is not an executable file; falling back to PATH lookup.`
    );
  }

  const onPath = findOnPath();
  if (onPath) return onPath;

  const cargoBin = path.join(os.homedir(), ".cargo", "bin", exeName());
  if (isExecutable(cargoBin)) return cargoBin;

  return findDownloaded(context.globalStorageUri.fsPath);
}

// ---------------------------------------------------------------------------
// GitHub release download
// ---------------------------------------------------------------------------

function httpsGet(url, opts = {}) {
  return new Promise((resolve, reject) => {
    const req = https.get(
      url,
      {
        headers: {
          "User-Agent": "canon-vscode",
          Accept: opts.accept || "application/octet-stream",
        },
      },
      (res) => {
        if (
          res.statusCode >= 300 &&
          res.statusCode < 400 &&
          res.headers.location
        ) {
          res.resume();
          resolve(httpsGet(res.headers.location, opts));
          return;
        }
        if (res.statusCode !== 200) {
          res.resume();
          reject(new Error(`GET ${url} -> HTTP ${res.statusCode}`));
          return;
        }
        const chunks = [];
        res.on("data", (c) => chunks.push(c));
        res.on("end", () => resolve(Buffer.concat(chunks)));
        res.on("error", reject);
      }
    );
    req.on("error", reject);
  });
}

async function downloadServer(context) {
  const target = releaseTarget();
  if (!target) {
    throw new Error(
      `no prebuilt canon binary for ${process.platform}/${process.arch}; build from source with \`cargo install --path .\``
    );
  }

  return vscode.window.withProgress(
    {
      location: vscode.ProgressLocation.Notification,
      title: "Canon: downloading language server",
      cancellable: false,
    },
    async (progress) => {
      progress.report({ message: "resolving latest release…" });
      const releaseJson = await httpsGet(
        `https://api.github.com/repos/${REPO}/releases/latest`,
        { accept: "application/vnd.github+json" }
      );
      const release = JSON.parse(releaseJson.toString("utf8"));
      const asset = (release.assets || []).find(
        (a) => a.name.includes(target) && a.name.endsWith(".tar.gz")
      );
      if (!asset) {
        throw new Error(
          `release ${release.tag_name} has no asset for ${target}`
        );
      }

      const storageDir = context.globalStorageUri.fsPath;
      fs.mkdirSync(storageDir, { recursive: true });
      const stem = asset.name.replace(/\.tar\.gz$/, "");
      const destDir = path.join(storageDir, stem);
      const archivePath = path.join(storageDir, asset.name);

      progress.report({ message: `downloading ${asset.name}…` });
      const bytes = await httpsGet(asset.browser_download_url);
      fs.writeFileSync(archivePath, bytes);

      progress.report({ message: "extracting…" });
      fs.mkdirSync(destDir, { recursive: true });
      // `tar` ships with macOS, every mainstream Linux, and Windows 10+.
      cp.execFileSync("tar", ["-xzf", archivePath, "-C", destDir]);
      fs.unlinkSync(archivePath);

      const nested = path.join(destDir, stem, exeName());
      const flat = path.join(destDir, exeName());
      const binary = fs.existsSync(nested) ? nested : flat;
      if (!fs.existsSync(binary)) {
        throw new Error(`extracted archive did not contain ${exeName()}`);
      }
      if (process.platform !== "win32") {
        fs.chmodSync(binary, 0o755);
      }
      return binary;
    }
  );
}

// ---------------------------------------------------------------------------
// Client lifecycle
// ---------------------------------------------------------------------------

async function startClient(context) {
  let binary = resolveBinary(context);

  if (!binary) {
    const choice = await vscode.window.showInformationMessage(
      "The `canon` binary was not found on PATH. Download a prebuilt binary from GitHub releases?",
      "Download",
      "Not now"
    );
    if (choice !== "Download") {
      return;
    }
    try {
      binary = await downloadServer(context);
    } catch (e) {
      vscode.window.showErrorMessage(
        `Canon: download failed (${e.message}). Install manually: https://github.com/${REPO}#installation`
      );
      return;
    }
  }

  const serverOptions = {
    command: binary,
    args: ["lsp"],
  };

  const clientOptions = {
    documentSelector: [{ scheme: "file", language: "canon" }],
  };

  client = new LanguageClient(
    "canon",
    "Canon Language Server",
    serverOptions,
    clientOptions
  );
  await client.start();
}

async function stopClient() {
  if (client) {
    const c = client;
    client = undefined;
    await c.stop();
  }
}

function activate(context) {
  context.subscriptions.push(
    vscode.commands.registerCommand("canon.restartServer", async () => {
      await stopClient();
      await startClient(context);
    }),
    vscode.commands.registerCommand("canon.downloadServer", async () => {
      try {
        const binary = await downloadServer(context);
        vscode.window.showInformationMessage(`Canon: downloaded ${binary}`);
        await stopClient();
        await startClient(context);
      } catch (e) {
        vscode.window.showErrorMessage(`Canon: download failed: ${e.message}`);
      }
    })
  );

  return startClient(context);
}

function deactivate() {
  return stopClient();
}

module.exports = { activate, deactivate };
