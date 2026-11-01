// Tests for the shared property-inspector rules in assets/propertyInspector/pi.js.
// Run with: node --test tests/*.test.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";

const require = createRequire(import.meta.url);
const { mergeSubset, clampInt, volumeLimit, applyCapabilities, brightnessPayload, audioNumbers, keepGesture, schedulePayload } = require(
	"../assets/propertyInspector/pi.js",
);

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

test("a volume cap lowers the offered maximum; no cap leaves it", () => {
	assert.equal(volumeLimit(100, 150), 100);
	assert.equal(volumeLimit(null, 150), 150);
	assert.equal(volumeLimit(undefined, 150), 150);
	assert.equal(volumeLimit(200, 150), 150);
});

test("pactl capabilities offer the App target and volume up to the bound", () => {
	const caps = { perApp: true, volumeCap: null, unsupportedNote: "" };
	assert.deepEqual(applyCapabilities(caps, 150), {
		appOption: { disabled: false, label: "Application" },
		volumeCap: null,
		volumeLimit: 150,
	});
});

test("CoreAudio capabilities disable the App target with the backend's note and cap volume", () => {
	const caps = { perApp: false, volumeCap: 100, unsupportedNote: "not supported on macOS" };
	assert.deepEqual(applyCapabilities(caps, 150), {
		appOption: { disabled: true, label: "Application (not supported on macOS)" },
		volumeCap: 100,
		volumeLimit: 100,
	});
});

test("no capabilities yet means everything is supported", () => {
	assert.deepEqual(applyCapabilities(undefined, 150).appOption, { disabled: false, label: "Application" });
	assert.equal(applyCapabilities(undefined, 150).volumeLimit, 150);
});

/** The shipped defaults and bounds: the page's `<script id="defaults">` JSON. */
function pageDefaults(page) {
	const html = readFileSync(new URL(`../assets/propertyInspector/${page}`, import.meta.url), "utf8");
	const json = html.match(/<script type="application\/json" id="defaults">([\s\S]*?)<\/script>/);
	assert.ok(json, `${page} has a defaults block`);
	return JSON.parse(json[1]);
}

const BRIGHTNESS = pageDefaults("brightness.html");
const AUDIO = pageDefaults("audio.html");

test("brightness: 300 saves 100 and selects pass through", () => {
	const p = brightnessPayload({ key_op: "set", strip_tap: "none", value: "300", step: "5", preset_a: "0", preset_b: "50" }, BRIGHTNESS);
	assert.deepEqual(p, { key_op: "set", strip_tap: "none", value: 100, step: 5, preset_a: 0, preset_b: 50 });
});

test("brightness: an empty or missing field saves the page default, never 0", () => {
	const empty = brightnessPayload({ key_op: "set", value: "" }, BRIGHTNESS);
	const missing = brightnessPayload({ key_op: "set" }, BRIGHTNESS);
	for (const p of [empty, missing]) {
		for (const id of Object.keys(BRIGHTNESS.bounds)) assert.equal(p[id], BRIGHTNESS.settings[id], id);
	}
	assert.notEqual(BRIGHTNESS.settings.step, 0);
});

test("audio: step 0 saves the lower bound and garbage the default", () => {
	const [minStep] = AUDIO.bounds.step;
	assert.equal(audioNumbers({ step: "0", max_volume: "100" }, AUDIO).step, minStep);
	assert.equal(audioNumbers({ step: "abc", max_volume: "100" }, AUDIO).step, AUDIO.settings.step);
});

test("audio: volumes are clamped to the bounds, lowered to the backend cap", () => {
	const values = { step: "5", max_volume: "150", gesture_volumes: { press: "999", long_press: "", touch_tap: "-5" } };
	const free = audioNumbers(values, AUDIO, { perApp: true, volumeCap: null, unsupportedNote: "" });
	assert.equal(free.max_volume, 150);
	assert.deepEqual(free.gesture_volumes, {
		press: 150,
		long_press: AUDIO.gestures.Keypad.press.volume,
		touch_tap: 0,
	});
	const capped = audioNumbers(values, AUDIO, { perApp: false, volumeCap: 100, unsupportedNote: "not supported on macOS" });
	assert.equal(capped.max_volume, 100);
	assert.equal(capped.gesture_volumes.press, 100);
	// No capabilities yet: no cap.
	assert.equal(audioNumbers(values, AUDIO).max_volume, 150);
});

test("a gesture slot left at the controller default stays unset", () => {
	const press = AUDIO.gestures.Keypad.press;
	assert.equal(keepGesture(undefined, { ...press }, press), undefined);
});

test("a gesture the user changed is stored", () => {
	const press = AUDIO.gestures.Keypad.press;
	const changed = { ...press, op: "none" };
	assert.deepEqual(keepGesture(undefined, changed, press), changed);
});

test("a stored gesture equal to the default is kept as stored", () => {
	const press = AUDIO.gestures.Keypad.press;
	const stored = { ...press };
	assert.equal(keepGesture(stored, { ...press }, press), stored);
});

test("a stored gesture set back to the default is unset again", () => {
	const press = AUDIO.gestures.Keypad.press;
	assert.equal(keepGesture({ ...press, op: "none" }, { ...press }, press), undefined);
});

test("schedule rows: HH:MM kept, anything else turns the slot off; values clamped", () => {
	const previous = [{ time: "", value: 10 }, { time: "", value: 70 }];
	assert.deepEqual(
		schedulePayload(
			[
				{ time: "23:00", value: "300" },
				{ time: "25:00", value: "" },
			],
			previous,
		),
		[
			{ time: "23:00", value: 100 },
			{ time: "", value: 70 },
		],
	);
	assert.deepEqual(schedulePayload([{ time: "", value: "abc" }], undefined), [{ time: "", value: 50 }]);
});
