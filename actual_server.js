import * as $ from 'svelte/internal/server';
import Option from './Option.svelte';


export default function Select_with_rich_content($$renderer, $$props = {}) {
	let items = [1, 2, 3];
	let show = true;
	let html = '<option>From HTML</option>';

	$$renderer.push(`

`);
	$$renderer.push(`<!-- select with rich option (has span inside) - SHOULD use customizable_select_element -->`);
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	$$renderer.push(`<option`);
	$$renderer.push(`>`);
	$$renderer.push(`<span`);
	$$renderer.push(`>`);
	$$renderer.push(`Rich`);
	$$renderer.push(`</span>`);
	$$renderer.push(`</option>`);
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- select with each containing plain options - should NOT use customizable_select_element -->`);
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	$$renderer.push(`<!--[-->`);
	const $$each_array_1 = $.ensure_array_like(items);
	for (let $$index_1 = 0, $$length_1 = $$each_array_1.length; $$index_1 < $$length_1; $$index_1++) {
		let item = $$each_array_1[$$index_1];
		$$renderer.push(`
		`);
		$$renderer.push(`<option`);
		$$renderer.push(`>`);
		$$renderer.push(`<!---->`);
		$$renderer.push($.escape(item));
		$$renderer.push(`</option>`);
		$$renderer.push(`
	`);
	}
	$$renderer.push(`<!--]-->`);
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- select with if containing plain options - should NOT use customizable_select_element -->`);
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	$$renderer.push(`<!--[-->`);
	if (show) {
		$$renderer.push(`<!--[0-->`);
		$$renderer.push(`
		`);
		$$renderer.push(`<option`);
		$$renderer.push(`>`);
		$$renderer.push(`Visible`);
		$$renderer.push(`</option>`);
		$$renderer.push(`
	`);
	}
	$$renderer.push(`<!--]-->`);
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- select with key containing plain options - should NOT use customizable_select_element -->`);
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	$$renderer.push(`<!---->`);
	$$renderer.push(`
		`);
	$$renderer.push(`<option`);
	$$renderer.push(`>`);
	$$renderer.push(`Keyed`);
	$$renderer.push(`</option>`);
	$$renderer.push(`
	`);
	$$renderer.push(`<!---->`);
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- select with snippet defined at top level and rendered - should NOT use customizable_select_element -->`);
	$$renderer.push(`
`);
	function opt($$renderer) {
		$$renderer.push(`
	`);
		$$renderer.push(`<option`);
		$$renderer.push(`>`);
		$$renderer.push(`Snippet`);
		$$renderer.push(`</option>`);
		$$renderer.push(`
`);
	}
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	(opt)($$renderer);
	$$renderer.push(`<!---->`);
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- select with const inside each (should be ignored) - should NOT use customizable_select_element -->`);
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	$$renderer.push(`<!--[-->`);
	const $$each_array_2 = $.ensure_array_like(items);
	for (let $$index_2 = 0, $$length_2 = $$each_array_2.length; $$index_2 < $$length_2; $$index_2++) {
		let item = $$each_array_2[$$index_2];
		$$renderer.push(`
		`);
		const x = item * 2;
		$$renderer.push(`
		`);
		$$renderer.push(`<option`);
		$$renderer.push(`>`);
		$$renderer.push(`<!---->`);
		$$renderer.push($.escape(x));
		$$renderer.push(`</option>`);
		$$renderer.push(`
	`);
	}
	$$renderer.push(`<!--]-->`);
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- optgroup with rich option - SHOULD use customizable_select_element -->`);
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	$$renderer.push(`<optgroup`);
	$$renderer.push(` label="`);
	$$renderer.push(`Group`);
	$$renderer.push(`"`);
	$$renderer.push(`>`);
	$$renderer.push(`
		`);
	$$renderer.push(`<option`);
	$$renderer.push(`>`);
	$$renderer.push(`<strong`);
	$$renderer.push(`>`);
	$$renderer.push(`Bold`);
	$$renderer.push(`</strong>`);
	$$renderer.push(`</option>`);
	$$renderer.push(`
	`);
	$$renderer.push(`</optgroup>`);
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- optgroup with each containing plain options - should NOT use customizable_select_element -->`);
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	$$renderer.push(`<optgroup`);
	$$renderer.push(` label="`);
	$$renderer.push(`Group`);
	$$renderer.push(`"`);
	$$renderer.push(`>`);
	$$renderer.push(`
		`);
	$$renderer.push(`<!--[-->`);
	const $$each_array_3 = $.ensure_array_like(items);
	for (let $$index_3 = 0, $$length_3 = $$each_array_3.length; $$index_3 < $$length_3; $$index_3++) {
		let item = $$each_array_3[$$index_3];
		$$renderer.push(`
			`);
		$$renderer.push(`<option`);
		$$renderer.push(`>`);
		$$renderer.push(`<!---->`);
		$$renderer.push($.escape(item));
		$$renderer.push(`</option>`);
		$$renderer.push(`
		`);
	}
	$$renderer.push(`<!--]-->`);
	$$renderer.push(`
	`);
	$$renderer.push(`</optgroup>`);
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- option with rich content (span) - SHOULD use customizable_select_element -->`);
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	$$renderer.push(`<option`);
	$$renderer.push(` value="`);
	$$renderer.push(`a`);
	$$renderer.push(`"`);
	$$renderer.push(`>`);
	$$renderer.push(`<em`);
	$$renderer.push(`>`);
	$$renderer.push(`Italic`);
	$$renderer.push(`</em>`);
	$$renderer.push(` text`);
	$$renderer.push(`</option>`);
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- nested: select > each > option with rich content - SHOULD use customizable_select_element on option -->`);
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	$$renderer.push(`<!--[-->`);
	const $$each_array_4 = $.ensure_array_like(items);
	for (let $$index_4 = 0, $$length_4 = $$each_array_4.length; $$index_4 < $$length_4; $$index_4++) {
		let item = $$each_array_4[$$index_4];
		$$renderer.push(`
		`);
		$$renderer.push(`<option`);
		$$renderer.push(`>`);
		$$renderer.push(`<span`);
		$$renderer.push(`>`);
		$$renderer.push(`<!---->`);
		$$renderer.push($.escape(item));
		$$renderer.push(`</span>`);
		$$renderer.push(`</option>`);
		$$renderer.push(`
	`);
	}
	$$renderer.push(`<!--]-->`);
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- nested: select > if > each > plain options - should NOT use customizable_select_element -->`);
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	$$renderer.push(`<!--[-->`);
	if (show) {
		$$renderer.push(`<!--[0-->`);
		$$renderer.push(`
		`);
		$$renderer.push(`<!--[-->`);
		const $$each_array_5 = $.ensure_array_like(items);
		for (let $$index_5 = 0, $$length_5 = $$each_array_5.length; $$index_5 < $$length_5; $$index_5++) {
			let item = $$each_array_5[$$index_5];
			$$renderer.push(`
			`);
			$$renderer.push(`<option`);
			$$renderer.push(`>`);
			$$renderer.push(`<!---->`);
			$$renderer.push($.escape(item));
			$$renderer.push(`</option>`);
			$$renderer.push(`
		`);
		}
		$$renderer.push(`<!--]-->`);
		$$renderer.push(`
	`);
	}
	$$renderer.push(`<!--]-->`);
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- select with svelte:boundary containing plain options - should NOT use customizable_select_element -->`);
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	$$renderer.boundary({}, ($$renderer) => {
		$$renderer.push(`
		`);
		$$renderer.push(`<option`);
		$$renderer.push(`>`);
		$$renderer.push(`Boundary`);
		$$renderer.push(`</option>`);
		$$renderer.push(`
	`);
	});
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- select with svelte:boundary containing rich options - SHOULD use customizable_select_element on option -->`);
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	$$renderer.boundary({}, ($$renderer) => {
		$$renderer.push(`
		`);
		$$renderer.push(`<option`);
		$$renderer.push(`>`);
		$$renderer.push(`<span`);
		$$renderer.push(`>`);
		$$renderer.push(`Rich in boundary`);
		$$renderer.push(`</span>`);
		$$renderer.push(`</option>`);
		$$renderer.push(`
	`);
	});
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- select with Component - SHOULD be treated as rich content -->`);
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	(Option)($$renderer, {});
	$$renderer.push(`<!---->`);
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- select with @render snippet - SHOULD be treated as rich content -->`);
	$$renderer.push(`
`);
	function option_snippet($$renderer) {
		$$renderer.push(`
	`);
		$$renderer.push(`<option`);
		$$renderer.push(`>`);
		$$renderer.push(`Rendered`);
		$$renderer.push(`</option>`);
		$$renderer.push(`
`);
	}
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	(option_snippet)($$renderer);
	$$renderer.push(`<!---->`);
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- select with @html - SHOULD be treated as rich content -->`);
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	$$renderer.push($.html(html));
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- optgroup with Component - SHOULD be treated as rich content -->`);
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	$$renderer.push(`<optgroup`);
	$$renderer.push(` label="`);
	$$renderer.push(`Group`);
	$$renderer.push(`"`);
	$$renderer.push(`>`);
	$$renderer.push(`
		`);
	(Option)($$renderer, {});
	$$renderer.push(`<!---->`);
	$$renderer.push(`
	`);
	$$renderer.push(`</optgroup>`);
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- optgroup with @render - SHOULD be treated as rich content -->`);
	$$renderer.push(`
`);
	function option_snippet2($$renderer) {
		$$renderer.push(`
	`);
		$$renderer.push(`<option`);
		$$renderer.push(`>`);
		$$renderer.push(`Rendered in group`);
		$$renderer.push(`</option>`);
		$$renderer.push(`
`);
	}
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	$$renderer.push(`<optgroup`);
	$$renderer.push(` label="`);
	$$renderer.push(`Group`);
	$$renderer.push(`"`);
	$$renderer.push(`>`);
	$$renderer.push(`
		`);
	(option_snippet2)($$renderer);
	$$renderer.push(`<!---->`);
	$$renderer.push(`
	`);
	$$renderer.push(`</optgroup>`);
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- option with @html inside - SHOULD use customizable_select_element -->`);
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	$$renderer.push(`<option`);
	$$renderer.push(`>`);
	$$renderer.push($.html('<strong>Bold HTML</strong>'));
	$$renderer.push(`</option>`);
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- each block inside select with Component - SHOULD be treated as rich -->`);
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	$$renderer.push(`<!--[-->`);
	const $$each_array_6 = $.ensure_array_like(items);
	for (let $$index_6 = 0, $$length_6 = $$each_array_6.length; $$index_6 < $$length_6; $$index_6++) {
		let item = $$each_array_6[$$index_6];
		$$renderer.push(`
		`);
		(Option)($$renderer, {});
		$$renderer.push(`<!---->`);
		$$renderer.push(`
	`);
	}
	$$renderer.push(`<!--]-->`);
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
	$$renderer.push(`

`);
	$$renderer.push(`<!-- if block inside select with @render - SHOULD be treated as rich -->`);
	$$renderer.push(`
`);
	function conditional_option($$renderer) {
		$$renderer.push(`
	`);
		$$renderer.push(`<option`);
		$$renderer.push(`>`);
		$$renderer.push(`Conditional`);
		$$renderer.push(`</option>`);
		$$renderer.push(`
`);
	}
	$$renderer.push(`
`);
	$$renderer.push(`<select`);
	$$renderer.push(`>`);
	$$renderer.push(`
	`);
	$$renderer.push(`<!--[-->`);
	if (show) {
		$$renderer.push(`<!--[0-->`);
		$$renderer.push(`
		`);
		(conditional_option)($$renderer);
		$$renderer.push(`<!---->`);
		$$renderer.push(`
	`);
	}
	$$renderer.push(`<!--]-->`);
	$$renderer.push(`
`);
	$$renderer.push(`</select>`);
}