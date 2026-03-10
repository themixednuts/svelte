import 'svelte/internal/flags/async';
import * as $ from 'svelte/internal/server';

export default function __COMPONENT__($$renderer) {
	function __COMPLEX_FN__() {
		return 1;
	}

	let __FOO_NAME__ = __FOO_INIT__;
	var __BLOCKING_NAME__;
	var $$promises = $$renderer.run([async () => __BLOCKING_NAME__ = await $.async_derived(() => __FOO_NAME__)]);

	$$renderer.async_block([$$promises[0]], ($$renderer) => {
		if (__FOO_NAME__) {
			$$renderer.push('<!--[0-->');
			$$renderer.push(`foo`);
		} else if (bar) {
			$$renderer.push('<!--[1-->');
			$$renderer.push(`bar`);
		} else {
			$$renderer.push('<!--[-1-->');
			$$renderer.push(`else`);
		}
	});

	$$renderer.push(`<!--]--> `);

	$$renderer.async_block([$$promises[0]], async ($$renderer) => {
		if ((await $.save(__FOO_NAME__))()) {
			$$renderer.push('<!--[0-->');
			$$renderer.push(`foo`);
		} else if (bar) {
			$$renderer.push('<!--[1-->');
			$$renderer.push(`bar`);
		} else {
			$$renderer.push('<!--[-1-->');

			$$renderer.child_block(async ($$renderer) => {
				if ((await $.save(baz))()) {
					$$renderer.push('<!--[0-->');
					$$renderer.push(`baz`);
				} else {
					$$renderer.push('<!--[-1-->');
					$$renderer.push(`else`);
				}
			});

			$$renderer.push(`<!--]-->`);
		}
	});

	$$renderer.push(`<!--]--> `);

	$$renderer.async_block([$$promises[0]], async ($$renderer) => {
		if ((await $.save(__FOO_NAME__))() > 10) {
			$$renderer.push('<!--[0-->');
			$$renderer.push(`foo`);
		} else if (bar) {
			$$renderer.push('<!--[1-->');
			$$renderer.push(`bar`);
		} else {
			$$renderer.push('<!--[-1-->');

			$$renderer.async_block([$$promises[0]], async ($$renderer) => {
				if ((await $.save(__FOO_NAME__))() > 5) {
					$$renderer.push('<!--[0-->');
					$$renderer.push(`baz`);
				} else {
					$$renderer.push('<!--[-1-->');
					$$renderer.push(`else`);
				}
			});

			$$renderer.push(`<!--]-->`);
		}
	});

	$$renderer.push(`<!--]--> `);

	if (simple1) {
		$$renderer.push('<!--[0-->');
		$$renderer.push(`foo`);
	} else if (simple2 > 10) {
		$$renderer.push('<!--[1-->');
		$$renderer.push(`bar`);
	} else if (__COMPLEX_FN__() * complex2 > 100) {
		$$renderer.push('<!--[2-->');
		$$renderer.push(`baz`);
	} else {
		$$renderer.push('<!--[-1-->');
		$$renderer.push(`else`);
	}

	$$renderer.push(`<!--]--> `);

	$$renderer.async_block([$$promises[0]], ($$renderer) => {
		if (__BLOCKING_NAME__() > 10) {
			$$renderer.push('<!--[0-->');
			$$renderer.push(`foo`);
		} else if (__BLOCKING_NAME__() > 5) {
			$$renderer.push('<!--[1-->');
			$$renderer.push(`bar`);
		} else {
			$$renderer.push('<!--[-1-->');
			$$renderer.push(`else`);
		}
	});

	$$renderer.push(`<!--]-->`);
}
