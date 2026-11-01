// Shared glue for this plugin's property inspectors: the OpenDeck / Stream
// Deck connection handshake, saving settings, and the input rules every page
// follows. Loaded as a classic script (window.PI); tests/pi.test.mjs loads it
// with require().
(function (root) {
	"use strict";

	/**
	 * The subset to store after the user ticked boxes. Only names that have a
	 * checkbox on screen are added or removed; stored names without one (an
	 * unplugged device, a list that hasn't arrived yet, the other device kind)
	 * are kept. Order: stored order first, newly ticked names appended.
	 */
	function mergeSubset(stored, rendered, ticked) {
		const kept = (stored || []).filter((name) => !rendered.includes(name) || ticked.includes(name));
		return [...kept, ...ticked.filter((name) => !kept.includes(name))];
	}

	/**
	 * A whole number clamped into [min, max]. Browsers don't enforce an
	 * input's min/max on "change", and an empty or garbled field means
	 * `fallback` (the setting's default), never 0.
	 */
	function clampInt(text, [min, max], fallback) {
		const n = Number.parseInt(text, 10);
		return Number.isFinite(n) ? Math.min(max, Math.max(min, n)) : fallback;
	}

	/**
	 * The highest volume to offer: a setting's upper `bound`, lowered to the
	 * backend's `cap` (100 on macOS) when there is one (null/undefined: none).
	 */
	function volumeLimit(cap, bound) {
		return Math.min(bound, cap ?? bound);
	}

	/**
	 * What the page does with the plugin's `capabilities` block: the App
	 * target option (kept so a saved App control still shows what it is,
	 * disabled with the backend's reason when there's no per-app audio) and
	 * the highest volume to offer below a setting's upper `bound`. A missing
	 * block means everything is supported.
	 */
	function applyCapabilities(caps, bound) {
		const perApp = caps?.perApp !== false;
		const note = caps?.unsupportedNote;
		return {
			appOption: {
				disabled: !perApp,
				label: perApp || !note ? "Application" : `Application (${note})`,
			},
			volumeCap: caps?.volumeCap ?? null,
			volumeLimit: volumeLimit(caps?.volumeCap, bound),
		};
	}

	/**
	 * The Device Brightness payload from the page's field `values` (strings
	 * as typed): selects pass through, every number in `defaults.bounds` is
	 * clamped (300 saves 100, an empty field saves the default, never 0).
	 */
	function brightnessPayload(values, defaults) {
		const payload = { key_op: values.key_op, strip_tap: values.strip_tap };
		for (const [id, bounds] of Object.entries(defaults.bounds)) {
			payload[id] = clampInt(values[id], bounds, defaults.settings[id]);
		}
		return payload;
	}

	/**
	 * The Audio page's numbers from its field `values` (strings as typed):
	 * `step`, `max_volume` and each gesture slot's `volume` (in
	 * `values.gesture_volumes`, by slot), clamped to `defaults.bounds` with
	 * the volume bounds lowered to the backend's cap (`caps`, the plugin's
	 * capabilities block; missing means no cap).
	 */
	function audioNumbers(values, defaults, caps) {
		const bounds = (name) => {
			const [min, max] = defaults.bounds[name];
			return [min, applyCapabilities(caps, max).volumeLimit];
		};
		const gestureDefault = defaults.gestures.Keypad.press.volume;
		return {
			step: clampInt(values.step, defaults.bounds.step, defaults.settings.step),
			max_volume: clampInt(values.max_volume, bounds("max_volume"), defaults.settings.max_volume),
			gesture_volumes: Object.fromEntries(
				Object.entries(values.gesture_volumes || {}).map(([slot, text]) => [
					slot,
					clampInt(text, bounds("gesture_volume"), gestureDefault),
				]),
			),
		};
	}

	/**
	 * What to store for one gesture slot after an edit elsewhere on the page.
	 * A slot left at the controller's default (`fallback`, what the plugin
	 * uses for an unset slot) stays unset, so a later change of default still
	 * reaches it; `stored` is kept when it already says the same. Anything
	 * else the user picked is stored as `collected`.
	 */
	function keepGesture(stored, collected, fallback) {
		const same = (a, b) => !!a && !!b && a.op === b.op && a.volume === b.volume && a.device === b.device;
		if (!same(collected, fallback)) return collected;
		return same(stored, collected) ? stored : undefined;
	}

	/** The JSON in `<script type="application/json" id="…">`. */
	function readJson(id) {
		return JSON.parse(document.getElementById(id).textContent);
	}

	/**
	 * Registers with the host. Handlers: `onOpen(payload)` once connected
	 * (payload has `controller` and `settings`), `onSettings(settings)`,
	 * `onMessage(payload)` for sendToPropertyInspector. Returns
	 * `{ save(settings), requestChoices() }`.
	 */
	function connect({ onOpen, onSettings, onMessage }) {
		let socket, uuid, action;
		const ready = new Promise((resolve) => {
			root.connectOpenActionSocket = (...args) => resolve(args);
			root.connectElgatoStreamDeckSocket = root.connectOpenActionSocket;
		});
		const send = (message) => socket && socket.send(JSON.stringify(message));
		ready.then(([port, inUUID, registerEvent, _info, actionInfoJson]) => {
			uuid = inUUID;
			const actionInfo = JSON.parse(actionInfoJson);
			action = actionInfo.action;
			socket = new WebSocket(`ws://127.0.0.1:${port}`);
			socket.onopen = () => {
				send({ event: registerEvent, uuid: inUUID });
				if (onOpen) onOpen(actionInfo.payload || {});
			};
			socket.onmessage = (event) => {
				const message = JSON.parse(event.data);
				if (message.event === "didReceiveSettings") {
					if (onSettings) onSettings(message.payload.settings || {});
				} else if (message.event === "sendToPropertyInspector") {
					if (onMessage) onMessage(message.payload || {});
				}
			};
		});
		return {
			save: (settings) => send({ event: "setSettings", context: uuid, payload: settings }),
			// Asked again once registered: the plugin's first push can arrive too early.
			requestChoices: () =>
				send({ event: "sendToPlugin", action, context: uuid, payload: { event: "requestChoices" } }),
		};
	}

	const api = {
		mergeSubset,
		clampInt,
		volumeLimit,
		applyCapabilities,
		brightnessPayload,
		audioNumbers,
		keepGesture,
		readJson,
		connect,
	};
	if (typeof module === "object" && module.exports) {
		module.exports = api;
	} else {
		root.PI = api;
	}
})(typeof window === "undefined" ? globalThis : window);
