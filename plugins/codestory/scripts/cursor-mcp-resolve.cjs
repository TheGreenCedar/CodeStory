"use strict";

function resolveCodestoryCursorLauncher(env, home, fs, path) {
  function launcherAt(root) {
    if (typeof root !== "string" || root.length === 0) return null;
    const candidate = path.join(root, "scripts", "codestory-mcp.cjs");
    try {
      return fs.statSync(candidate).isFile() ? fs.realpathSync(candidate) : null;
    } catch {
      return null;
    }
  }

  function isCodestoryPlugin(root) {
    try {
      const manifest = JSON.parse(fs.readFileSync(path.join(root, "plugin.json"), "utf8"));
      return manifest && manifest.name === "codestory";
    } catch {
      return false;
    }
  }

  function fromRoot(root) {
    const launcher = launcherAt(root);
    return launcher && isCodestoryPlugin(root) ? launcher : null;
  }

  function fromCache() {
    const cacheRoot = path.join(home, ".cursor", "plugins", "cache");
    let best = null;
    let bestMtime = -1;
    let marketplaces;
    try {
      marketplaces = fs.readdirSync(cacheRoot);
    } catch {
      return null;
    }
    for (const marketName of marketplaces) {
      const marketPath = path.join(cacheRoot, marketName);
      let pluginNames;
      try {
        if (!fs.statSync(marketPath).isDirectory()) continue;
        pluginNames = fs.readdirSync(marketPath);
      } catch {
        continue;
      }
      for (const pluginName of pluginNames) {
        if (pluginName !== "codestory") continue;
        const pluginPath = path.join(marketPath, pluginName);
        let revisions;
        try {
          if (!fs.statSync(pluginPath).isDirectory()) continue;
          revisions = fs.readdirSync(pluginPath);
        } catch {
          continue;
        }
        for (const revision of revisions) {
          const root = path.join(pluginPath, revision);
          const launcher = fromRoot(root);
          if (!launcher) continue;
          let mtime = 0;
          try {
            mtime = fs.statSync(launcher).mtimeMs;
          } catch {
            mtime = 0;
          }
          if (mtime >= bestMtime) {
            best = launcher;
            bestMtime = mtime;
          }
        }
      }
    }
    return best;
  }

  return fromRoot(env.CURSOR_PLUGIN_ROOT)
    || fromRoot(path.join(home, ".cursor", "plugins", "local", "codestory"))
    || fromCache()
    || fromRoot(env.PLUGIN_ROOT)
    || (() => {
      throw new Error("codestory_cursor_mcp_launcher_not_found");
    })();
}

const INLINE_ENTRY = `'use strict';const fs=require('fs');const os=require('os');const path=require('path');${resolveCodestoryCursorLauncher.toString()}require(resolveCodestoryCursorLauncher(process.env,os.homedir(),fs,path));`;

if (require.main === module) {
  require(resolveCodestoryCursorLauncher(
    process.env,
    require("os").homedir(),
    require("fs"),
    require("path"),
  ));
}

module.exports = {
  INLINE_ENTRY,
  resolveCodestoryCursorLauncher,
};
