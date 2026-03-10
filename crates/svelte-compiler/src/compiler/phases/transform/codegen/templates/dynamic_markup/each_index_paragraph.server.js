import * as $ from 'svelte/internal/server';

export default function __COMPONENT__($$renderer) {
	$$renderer.push(`<!--[-->`);

	const each_array = $.ensure_array_like(__COLLECTION__);

	for (let __INDEX__ = 0, $$length = each_array.length; __INDEX__ < $$length; __INDEX__++) {
		$$renderer.push(`<p>index: ${$.escape(__INDEX__)}</p>`);
	}

	$$renderer.push(`<!--]-->`);
}
