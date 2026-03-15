import 'svelte/internal/disclose-version';
import 'svelte/internal/flags/legacy';
import * as $ from 'svelte/internal/client';

var root = $.from_html(`<input/>`);

export default function __COMPONENT__($$anchor, $$props) {
	$.push($$props, false);

	let __PROP__ = $.prop($$props, __PROP_LITERAL__, 12);

	$.init();

	var input = root();

	$.remove_input_defaults(input);
	$.bind_value(input, () => __GETTER_EXPR__, ($$value) => __SETTER_EXPR__);
	$.append($$anchor, input);
	$.pop();
}
