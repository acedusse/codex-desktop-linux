#!/usr/bin/env node
"use strict";

const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");

const patcher = path.join(__dirname, "patch-chrome-plugin.js");

function writeScript(pluginDir, name, source) {
  const scriptsDir = path.join(pluginDir, "scripts");
  fs.mkdirSync(scriptsDir, { recursive: true });
  fs.writeFileSync(path.join(scriptsDir, name), source, "utf8");
}

function readScript(pluginDir, name) {
  return fs.readFileSync(path.join(pluginDir, "scripts", name), "utf8");
}

test("patches Linux Chrome Beta and Unstable support into bundled Chrome plugin scripts", () => {
  const pluginDir = fs.mkdtempSync(path.join(os.tmpdir(), "codex-chrome-plugin-"));
  try {
    writeScript(
      pluginDir,
      "installManifest.mjs",
      'const hostPlatforms={linux:[".config/google-chrome/NativeMessagingHosts"]};\n',
    );
    writeScript(
      pluginDir,
      "check-native-host-manifest.js",
      `const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
function getNativeHostManifestDetails(expectedHostName) {
  if (process.platform === "linux") {
    return {
      manifestPath: path.join(
        os.homedir(),
        ".config",
        "google-chrome",
        "NativeMessagingHosts",
        \`\${expectedHostName}.json\`,
      ),
      registryKey: null,
      registryManifestPath: null,
      registryKeyExists: null,
    };
  }
}
`,
    );
    writeScript(
      pluginDir,
      "browser-client.mjs",
      String.raw`var Tc=GF(VF(),WF()==="win32"?"AppData\\Local\\Google\\Chrome\\User Data":"Library/Application Support/Google/Chrome");` +
        "\n",
    );
    writeScript(
      pluginDir,
      "installed-browsers.js",
      `const KNOWN_BROWSERS = [
  {
    name: "Google Chrome",
    bundleIds: ["com.google.Chrome"],
    appNames: ["Google Chrome.app"],
    commands: ["google-chrome", "chrome"],
    windowsExecutable: "chrome.exe",
  },
];
`,
    );
    writeScript(
      pluginDir,
      "chrome-is-running.js",
      `const CHROME_PROCESS_NAMES_BY_PLATFORM = {
  darwin: new Set(["Google Chrome", "Google Chrome Helper"]),
  win32: new Set(["chrome.exe"]),
};
`,
    );
    writeScript(
      pluginDir,
      "check-extension-installed.js",
      `const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
function resolveChromeUserDataDirectory() {
  return path.join(os.homedir(), ".config", "google-chrome");
}
function resolveChromeProfileDirectory(userDataDirectory) {
  const localStateProfile =
    resolveChromeProfileDirectoryFromLocalState(userDataDirectory);
  if (localStateProfile) return localStateProfile;
}
function resolveChromeProfileDirectoryFromLocalState(userDataDirectory) {
  return null;
}
`,
    );
    writeScript(
      pluginDir,
      "open-chrome-window.js",
      `const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
function runCommand() {
  return "";
}
function commandPath(command) {
  return command;
}
function resolveChromeUserDataDirectory() {
  return path.join(os.homedir(), ".config", "google-chrome");
}
function resolveChromeProfileDirectory(userDataDirectory) {
  const localStateProfile =
    resolveChromeProfileDirectoryFromLocalState(userDataDirectory);
  if (localStateProfile) return localStateProfile;
}
function resolveChromeProfileDirectoryFromLocalState(userDataDirectory) {
  return null;
}
function openChromeWindow(chromeArgs) {
  return {
    command: "google-chrome",
    args: chromeArgs,
  };
}
`,
    );

    const result = spawnSync(process.execPath, [patcher, pluginDir], {
      encoding: "utf8",
    });
    assert.equal(result.status, 0, result.stderr);

    const installManifest = readScript(pluginDir, "installManifest.mjs");
    assert.match(installManifest, /google-chrome-beta\/NativeMessagingHosts/);
    assert.match(installManifest, /google-chrome-unstable\/NativeMessagingHosts/);

    const nativeHostCheck = readScript(pluginDir, "check-native-host-manifest.js");
    assert.match(nativeHostCheck, /"google-chrome-beta",\n        "NativeMessagingHosts"/);
    assert.match(nativeHostCheck, /"google-chrome-unstable",\n        "NativeMessagingHosts"/);

    const browserClient = readScript(pluginDir, "browser-client.mjs");
    assert.match(browserClient, /"google-chrome-beta"/);
    assert.match(browserClient, /"google-chrome-unstable"/);

    const installedBrowsers = readScript(pluginDir, "installed-browsers.js");
    assert.match(installedBrowsers, /name: "Google Chrome Beta"/);
    assert.match(installedBrowsers, /commands: \["google-chrome-beta"\]/);
    assert.match(installedBrowsers, /name: "Google Chrome Unstable"/);
    assert.match(installedBrowsers, /commands: \["google-chrome-unstable"\]/);

    const runningCheck = readScript(pluginDir, "chrome-is-running.js");
    assert.match(runningCheck, /"google-chrome-beta"/);
    assert.match(runningCheck, /"google-chrome-unstable"/);

    const extensionCheck = readScript(pluginDir, "check-extension-installed.js");
    assert.match(extensionCheck, /linuxChromeBetaUserDataDirectory/);
    assert.match(extensionCheck, /"google-chrome-beta"/);
    assert.match(extensionCheck, /"google-chrome-unstable"/);

    const openWindow = readScript(pluginDir, "open-chrome-window.js");
    assert.match(openWindow, /google-chrome-beta\.desktop/);
    assert.match(openWindow, /commandPath\("google-chrome-beta"\)/);
    assert.match(openWindow, /commandPath\("google-chrome-unstable"\)/);
  } finally {
    fs.rmSync(pluginDir, { recursive: true, force: true });
  }
});
