import * as $client from 'svelte/internal/client';
import * as $server from 'svelte/internal/server';
const $ = { ...$client, ...$server };
__MODULE_BODY__

export default function __COMPONENT__($$anchor, $$props = {}) {
__INSTANCE_BODY__
	let $$buffer = '';
	const $$flush = () => {
		if ($$buffer.length === 0) {
			return;
		}
		var $$fragment = $client.comment();
		$client.append($$anchor, $$fragment);
		$client.html($client.first_child($$fragment), () => $$buffer);
		$$buffer = '';
	};
	const $$renderer = {
		push(chunk) {
			if (typeof chunk === 'function') {
				chunk = chunk();
			}
			$$buffer += String(chunk ?? '');
		},
		title(fn) {
			fn($$renderer);
		},
		boundary(_props, fn) {
			fn($$renderer);
		},
		child(fn) {
			fn($$renderer);
			return $$renderer;
		},
		component(fn) {
			fn($$renderer);
		},
		flush: $$flush
	};
__BODY__
	$$flush();
}
