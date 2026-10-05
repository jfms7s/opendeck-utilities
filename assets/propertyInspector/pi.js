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

	const api = { mergeSubset, clampInt, readJson, connect };
	if (typeof module === "object" && module.exports) {
		module.exports = api;
	} else {
		root.PI = api;
	}
})(typeof window === "undefined" ? globalThis : window);
