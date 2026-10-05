#!/usr/bin/env node
// Assembles dist/<uuid>.sdPlugin/ from assets/ and the release binaries.
// Usage: node build.mjs [target-triple...]
//   With triples: packages exactly those (each must be built).
//   Without: packages every target in manifest.json's CodePaths that is built.
// Build first: cargo build --release --locked --target <target-triple>
// Binary names come from manifest.json's CodePaths, the single source of truth.
import { cpSync, copyFileSync, mkdirSync, rmSync, existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

const UUID = "com.jfms7s.utilities";
const BIN_NAME = "opendeck-utilities";

function fail(message) {
	console.error(message);
	process.exit(1);
}

// Cargo.toml's [package] version and manifest.json's "Version" have nothing
// keeping them in sync - catch drift here rather than shipping a plugin
// whose crate version and Elgato-facing manifest version disagree.
const cargoToml = readFileSync("Cargo.toml", "utf8");
const cargoVersionMatch = cargoToml.match(/^version\s*=\s*"([^"]+)"/m);
if (!cargoVersionMatch) fail('could not find `version = "..."` in Cargo.toml');
const cargoVersion = cargoVersionMatch[1];

const manifest = JSON.parse(readFileSync("assets/manifest.json", "utf8"));
if (cargoVersion !== manifest.Version) {
	fail(
		`version mismatch: Cargo.toml is ${cargoVersion} but assets/manifest.json is ${manifest.Version} - bump them together`,
	);
}

const codePaths = manifest.CodePaths || {};
const builtBinary = (target) => join("target", target, "release", BIN_NAME);

const requested = process.argv.slice(2);
for (const target of requested) {
	if (!codePaths[target]) fail(`${target} is not in manifest.json's CodePaths`);
	if (!existsSync(builtBinary(target))) {
		fail(`missing release binary: ${builtBinary(target)} (run: cargo build --release --locked --target ${target})`);
	}
}
const targets = requested.length > 0 ? requested : Object.keys(codePaths).filter((t) => existsSync(builtBinary(t)));
if (targets.length === 0) {
	fail(`no release binaries found; build one of: ${Object.keys(codePaths).join(", ")}`);
}

const outDir = join("dist", `${UUID}.sdPlugin`);
rmSync(outDir, { recursive: true, force: true });
mkdirSync(outDir, { recursive: true });

cpSync("assets/manifest.json", join(outDir, "manifest.json"));
// Not every plugin has layouts or a property inspector - copy what exists.
// Icon sources (assets/icon-src/) are deliberately not shipped.
for (const dir of ["icons", "layouts", "propertyInspector"]) {
	if (existsSync(join("assets", dir))) {
		cpSync(join("assets", dir), join(outDir, dir), { recursive: true });
	}
}
for (const target of targets) {
	copyFileSync(builtBinary(target), join(outDir, codePaths[target]));
}

const missing = Object.keys(codePaths).filter((t) => !targets.includes(t));
console.log(`built ${outDir} for ${targets.join(", ")}`);
if (missing.length > 0) console.log(`note: no binary for ${missing.join(", ")} (fine for a local build)`);
