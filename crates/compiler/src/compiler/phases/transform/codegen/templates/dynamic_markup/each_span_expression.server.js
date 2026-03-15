import * as $ from 'svelte/internal/server';

export default function __COMPONENT__($$renderer) {
	$$renderer.push(`<!--[-->`);

	const each_array = $.ensure_array_like(__COLLECTION__);

	for (let $$index = 0, $$length = each_array.length; $$index < $$length; $$index++) {
		let __CONTEXT__ = each_array[$$index];

		$$renderer.push(`<span>${$.escape(__CONTEXT__)}</span>`);
	}

	$$renderer.push(`<!--]-->`);
}
