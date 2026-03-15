import 'svelte/internal/disclose-version';
import 'svelte/internal/flags/legacy';
import * as $ from 'svelte/internal/client';

var root_1 = $.from_html(`<p></p>`);

export default function __COMPONENT__($$anchor) {
	var fragment = $.comment();
	var node = $.first_child(fragment);

	$.each(node, 0, () => __COLLECTION__, $.index, ($$anchor, $$item, __INDEX__) => {
		var p = root_1();

		p.textContent = `index: ${__INDEX__}`;
		$.append($$anchor, p);
	});

	$.append($$anchor, fragment);
}
