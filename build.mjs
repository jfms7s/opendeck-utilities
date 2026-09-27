#!/usr/bin/env node
// Assembles dist/<uuid>.sdPlugin/ from assets/ + a release binary for one target.
// Usage: node build.mjs <target-triple>
// Requires: cargo build --release --target <target-triple> already run for that triple.
import { cpSync, copyFileSync, mkdirSync, rmSync, existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

const UUID = "com.jfms7s.utilities";
const BIN_NAME = "opendeck-utilities";

const target = process.argv[2];
if (!target) {
	console.error("usage: node build.mjs <target-triple>");
	process.exit(1);
}

// Cargo.toml's version and manifest.json's "Version" have nothing keeping
// them in sync - catch drift here rather than shipping a mismatch.
const cargoVersion = readFileSync("Cargo.toml", "utf8").match(/^version\s*=\s*"([^"]+)"/m)?.[1];
const manifestVersion = JSON.parse(readFileSync("assets/manifest.json", "utf8")).Version;
if (!cargoVersion || cargoVersion !== manifestVersion) {
	console.error(`version mismatch: Cargo.toml is ${cargoVersion} but assets/manifest.json is ${manifestVersion} - bump them together`);
	process.exit(1);
}

const binPath = join("target", target, "release", BIN_NAME);
if (!existsSync(binPath)) {
	console.error(`missing release binary: ${binPath} (run: cargo build --release --target ${target})`);
	process.exit(1);
}

const outDir = join("dist", `${UUID}.sdPlugin`);
rmSync(outDir, { recursive: true, force: true });
mkdirSync(outDir, { recursive: true });

cpSync("assets/manifest.json", join(outDir, "manifest.json"));
for (const dir of ["icons", "layouts", "propertyInspector"]) {
	if (existsSync(join("assets", dir))) {
		cpSync(join("assets", dir), join(outDir, dir), { recursive: true });
	}
}
copyFileSync(binPath, join(outDir, `${BIN_NAME}-${target}`));

console.log(`built ${outDir} for ${target}`);
