import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { CodeXml } from "lucide-react";

const require = createRequire(import.meta.url);
const sharpRoot = process.env.CODEX_NODE_MODULES;
if (!sharpRoot) {
  throw new Error("CODEX_NODE_MODULES is required");
}
const sharp = require(path.join(sharpRoot, "sharp"));
const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const output = path.join(projectRoot, "src-tauri", "icons", "app-icon.png");

const iconMarkup = renderToStaticMarkup(
  React.createElement(CodeXml, {
    width: 300,
    height: 300,
    color: "#ffffff",
    strokeWidth: 2.15,
  }),
);

await sharp({
  create: {
    width: 512,
    height: 512,
    channels: 4,
    background: "#176ff2",
  },
})
  .composite([
    {
      input: Buffer.from(iconMarkup),
      left: 106,
      top: 106,
    },
  ])
  .png()
  .toFile(output);

console.log(output);
