import * as $ from 'svelte/internal/server';

export default function __COMPONENT__($$renderer, $$props) {
	$$renderer.component(($$renderer) => {
		let __PROP__ = $$props[__PROP_LITERAL__];

		$$renderer.push(`<input${$.attr('value', __MEMBER_EXPR__)}/>`);
		$.bind_props($$props, { __PROP__ });
	});
}
