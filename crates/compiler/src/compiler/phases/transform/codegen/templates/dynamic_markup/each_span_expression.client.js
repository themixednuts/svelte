import 'svelte/internal/disclose-version';
import 'svelte/internal/flags/legacy';
import * as $ from 'svelte/internal/client';

var root_1 = $.from_html(`<span> </span>`);

export default function __COMPONENT__($$anchor) {
	var fragment = $.comment();
	var node = $.first_child(fragment);

	$.each(node, 0, () => __COLLECTION__, $.index, ($$anchor, __CONTEXT__) => {
		var span = root_1();
		var text = $.child(span, true);

		$.reset(span);
		$.template_effect(() => $.set_text(text, __CONTEXT__));
		$.append($$anchor, span);
	});

	$.append($$anchor, fragment);
}
