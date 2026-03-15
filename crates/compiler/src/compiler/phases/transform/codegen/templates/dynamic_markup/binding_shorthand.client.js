import 'svelte/internal/disclose-version';
import 'svelte/internal/flags/legacy';
import * as $ from 'svelte/internal/client';

var root = $.from_html(` <!>`, 1);

export default function __COMPONENT__($$anchor, $$props) {
	let __PROP__ = $.prop($$props, __PROP_LITERAL__, 12);

	$.next();

	var fragment = root();
	var text = $.first_child(fragment);
	var node = $.sibling(text);

	__CHILD_COMPONENT__(node, {
		get __PROP__() {
			return __PROP__();
		},

		set __PROP__($$value) {
			__PROP__($$value);
		},
		$$legacy: true
	});

	$.template_effect(() => $.set_text(text, `${__PROP__() ?? ''} `));
	$.append($$anchor, fragment);
}
