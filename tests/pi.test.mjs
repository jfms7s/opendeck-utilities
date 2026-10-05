// Tests for the shared property-inspector rules in assets/propertyInspector/pi.js.
// Run with: node --test tests/
import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { mergeSubset, clampInt } = require("../assets/propertyInspector/pi.js");

test("a stored name with no checkbox survives an unrelated save", () => {
	// Headset H is unplugged, so only A and B are listed; the user edits the label.
	assert.deepEqual(mergeSubset(["A", "H"], ["A", "B"], ["A"]), ["A", "H"]);
});

test("saving before the choices arrive keeps the whole subset", () => {
	assert.deepEqual(mergeSubset(["A", "H"], [], []), ["A", "H"]);
});

test("unticking a listed name removes it", () => {
	assert.deepEqual(mergeSubset(["A", "B"], ["A", "B"], ["B"]), ["B"]);
});

test("newly ticked names are appended in tick order", () => {
	assert.deepEqual(mergeSubset(["B"], ["A", "B", "C"], ["C", "A", "B"]), ["B", "C", "A"]);
	assert.deepEqual(mergeSubset(undefined, ["A"], ["A"]), ["A"]);
});

test("numbers clamp into range and blanks fall back to the default", () => {
	assert.equal(clampInt("300", [0, 100], 50), 100);
	assert.equal(clampInt("-5", [1, 50], 5), 1);
	assert.equal(clampInt("0", [0, 100], 50), 0);
	assert.equal(clampInt("", [0, 100], 50), 50);
	assert.equal(clampInt("abc", [1, 20], 5), 5);
});
