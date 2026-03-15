import 'svelte/internal/disclose-version';
import 'svelte/internal/flags/legacy';
import * as $ from 'svelte/internal/client';

export default function __COMPONENT__($$anchor) {
	var fragment = $.comment();
	var node = $.first_child(fragment);

	$.each(node, 0, () => __COLLECTION__, $.index, ($$anchor, __CONTEXT__) => {
		$.next();

		var text = $.text();

		$.template_effect(() => $.set_text(text, `${__CONTEXT__ ?? ''}, `));
		$.append($$anchor, text);
	});

	$.append($$anchor, fragment);
}
