import * as $ from 'svelte/internal/server';

export default function __COMPONENT__($$renderer, $$props) {
	let __PROP__ = $$props[__PROP_LITERAL__];
	let $$settled = true;
	let $$inner_renderer;

	function $$render_inner($$renderer) {
		$$renderer.push(`<!---->${$.escape(__PROP__)} `);

		__CHILD_COMPONENT__($$renderer, {
			get __PROP__() {
				return __PROP__;
			},

			set __PROP__($$value) {
				__PROP__ = $$value;
				$$settled = false;
			}
		});

		$$renderer.push(`<!---->`);
	}

	do {
		$$settled = true;
		$$inner_renderer = $$renderer.copy();
		$$render_inner($$inner_renderer);
	} while (!$$settled);

	$$renderer.subsume($$inner_renderer);
	$.bind_props($$props, { __PROP__ });
}
