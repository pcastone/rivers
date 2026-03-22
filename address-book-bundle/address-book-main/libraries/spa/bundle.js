var app = (function () {
	'use strict';

	/** @returns {void} */
	function noop() {}

	function run(fn) {
		return fn();
	}

	function blank_object() {
		return Object.create(null);
	}

	/**
	 * @param {Function[]} fns
	 * @returns {void}
	 */
	function run_all(fns) {
		fns.forEach(run);
	}

	/**
	 * @param {any} thing
	 * @returns {thing is Function}
	 */
	function is_function(thing) {
		return typeof thing === 'function';
	}

	/** @returns {boolean} */
	function safe_not_equal(a, b) {
		return a != a ? b == b : a !== b || (a && typeof a === 'object') || typeof a === 'function';
	}

	let src_url_equal_anchor;

	/**
	 * @param {string} element_src
	 * @param {string} url
	 * @returns {boolean}
	 */
	function src_url_equal(element_src, url) {
		if (element_src === url) return true;
		if (!src_url_equal_anchor) {
			src_url_equal_anchor = document.createElement('a');
		}
		// This is actually faster than doing URL(..).href
		src_url_equal_anchor.href = url;
		return element_src === src_url_equal_anchor.href;
	}

	/** @returns {boolean} */
	function is_empty(obj) {
		return Object.keys(obj).length === 0;
	}

	/**
	 * @param {Node} target
	 * @param {Node} node
	 * @returns {void}
	 */
	function append(target, node) {
		target.appendChild(node);
	}

	/**
	 * @param {Node} target
	 * @param {Node} node
	 * @param {Node} [anchor]
	 * @returns {void}
	 */
	function insert(target, node, anchor) {
		target.insertBefore(node, anchor || null);
	}

	/**
	 * @param {Node} node
	 * @returns {void}
	 */
	function detach(node) {
		if (node.parentNode) {
			node.parentNode.removeChild(node);
		}
	}

	/**
	 * @template {keyof HTMLElementTagNameMap} K
	 * @param {K} name
	 * @returns {HTMLElementTagNameMap[K]}
	 */
	function element(name) {
		return document.createElement(name);
	}

	/**
	 * @param {string} data
	 * @returns {Text}
	 */
	function text(data) {
		return document.createTextNode(data);
	}

	/**
	 * @returns {Text} */
	function space() {
		return text(' ');
	}

	/**
	 * @returns {Text} */
	function empty() {
		return text('');
	}

	/**
	 * @param {EventTarget} node
	 * @param {string} event
	 * @param {EventListenerOrEventListenerObject} handler
	 * @param {boolean | AddEventListenerOptions | EventListenerOptions} [options]
	 * @returns {() => void}
	 */
	function listen(node, event, handler, options) {
		node.addEventListener(event, handler, options);
		return () => node.removeEventListener(event, handler, options);
	}

	/**
	 * @param {Element} node
	 * @param {string} attribute
	 * @param {string} [value]
	 * @returns {void}
	 */
	function attr(node, attribute, value) {
		if (value == null) node.removeAttribute(attribute);
		else if (node.getAttribute(attribute) !== value) node.setAttribute(attribute, value);
	}

	/**
	 * @param {Element} element
	 * @returns {ChildNode[]}
	 */
	function children(element) {
		return Array.from(element.childNodes);
	}

	/**
	 * @param {Text} text
	 * @param {unknown} data
	 * @returns {void}
	 */
	function set_data(text, data) {
		data = '' + data;
		if (text.data === data) return;
		text.data = /** @type {string} */ (data);
	}

	/**
	 * @returns {void} */
	function set_input_value(input, value) {
		input.value = value == null ? '' : value;
	}

	/**
	 * @typedef {Node & {
	 * 	claim_order?: number;
	 * 	hydrate_init?: true;
	 * 	actual_end_child?: NodeEx;
	 * 	childNodes: NodeListOf<NodeEx>;
	 * }} NodeEx
	 */

	/** @typedef {ChildNode & NodeEx} ChildNodeEx */

	/** @typedef {NodeEx & { claim_order: number }} NodeEx2 */

	/**
	 * @typedef {ChildNodeEx[] & {
	 * 	claim_info?: {
	 * 		last_index: number;
	 * 		total_claimed: number;
	 * 	};
	 * }} ChildNodeArray
	 */

	let current_component;

	/** @returns {void} */
	function set_current_component(component) {
		current_component = component;
	}

	const dirty_components = [];
	const binding_callbacks = [];

	let render_callbacks = [];

	const flush_callbacks = [];

	const resolved_promise = /* @__PURE__ */ Promise.resolve();

	let update_scheduled = false;

	/** @returns {void} */
	function schedule_update() {
		if (!update_scheduled) {
			update_scheduled = true;
			resolved_promise.then(flush);
		}
	}

	/** @returns {void} */
	function add_render_callback(fn) {
		render_callbacks.push(fn);
	}

	// flush() calls callbacks in this order:
	// 1. All beforeUpdate callbacks, in order: parents before children
	// 2. All bind:this callbacks, in reverse order: children before parents.
	// 3. All afterUpdate callbacks, in order: parents before children. EXCEPT
	//    for afterUpdates called during the initial onMount, which are called in
	//    reverse order: children before parents.
	// Since callbacks might update component values, which could trigger another
	// call to flush(), the following steps guard against this:
	// 1. During beforeUpdate, any updated components will be added to the
	//    dirty_components array and will cause a reentrant call to flush(). Because
	//    the flush index is kept outside the function, the reentrant call will pick
	//    up where the earlier call left off and go through all dirty components. The
	//    current_component value is saved and restored so that the reentrant call will
	//    not interfere with the "parent" flush() call.
	// 2. bind:this callbacks cannot trigger new flush() calls.
	// 3. During afterUpdate, any updated components will NOT have their afterUpdate
	//    callback called a second time; the seen_callbacks set, outside the flush()
	//    function, guarantees this behavior.
	const seen_callbacks = new Set();

	let flushidx = 0; // Do *not* move this inside the flush() function

	/** @returns {void} */
	function flush() {
		// Do not reenter flush while dirty components are updated, as this can
		// result in an infinite loop. Instead, let the inner flush handle it.
		// Reentrancy is ok afterwards for bindings etc.
		if (flushidx !== 0) {
			return;
		}
		const saved_component = current_component;
		do {
			// first, call beforeUpdate functions
			// and update components
			try {
				while (flushidx < dirty_components.length) {
					const component = dirty_components[flushidx];
					flushidx++;
					set_current_component(component);
					update(component.$$);
				}
			} catch (e) {
				// reset dirty state to not end up in a deadlocked state and then rethrow
				dirty_components.length = 0;
				flushidx = 0;
				throw e;
			}
			set_current_component(null);
			dirty_components.length = 0;
			flushidx = 0;
			while (binding_callbacks.length) binding_callbacks.pop()();
			// then, once components are updated, call
			// afterUpdate functions. This may cause
			// subsequent updates...
			for (let i = 0; i < render_callbacks.length; i += 1) {
				const callback = render_callbacks[i];
				if (!seen_callbacks.has(callback)) {
					// ...so guard against infinite loops
					seen_callbacks.add(callback);
					callback();
				}
			}
			render_callbacks.length = 0;
		} while (dirty_components.length);
		while (flush_callbacks.length) {
			flush_callbacks.pop()();
		}
		update_scheduled = false;
		seen_callbacks.clear();
		set_current_component(saved_component);
	}

	/** @returns {void} */
	function update($$) {
		if ($$.fragment !== null) {
			$$.update();
			run_all($$.before_update);
			const dirty = $$.dirty;
			$$.dirty = [-1];
			$$.fragment && $$.fragment.p($$.ctx, dirty);
			$$.after_update.forEach(add_render_callback);
		}
	}

	/**
	 * Useful for example to execute remaining `afterUpdate` callbacks before executing `destroy`.
	 * @param {Function[]} fns
	 * @returns {void}
	 */
	function flush_render_callbacks(fns) {
		const filtered = [];
		const targets = [];
		render_callbacks.forEach((c) => (fns.indexOf(c) === -1 ? filtered.push(c) : targets.push(c)));
		targets.forEach((c) => c());
		render_callbacks = filtered;
	}

	const outroing = new Set();

	/**
	 * @type {Outro}
	 */
	let outros;

	/**
	 * @returns {void} */
	function group_outros() {
		outros = {
			r: 0,
			c: [],
			p: outros // parent group
		};
	}

	/**
	 * @returns {void} */
	function check_outros() {
		if (!outros.r) {
			run_all(outros.c);
		}
		outros = outros.p;
	}

	/**
	 * @param {import('./private.js').Fragment} block
	 * @param {0 | 1} [local]
	 * @returns {void}
	 */
	function transition_in(block, local) {
		if (block && block.i) {
			outroing.delete(block);
			block.i(local);
		}
	}

	/**
	 * @param {import('./private.js').Fragment} block
	 * @param {0 | 1} local
	 * @param {0 | 1} [detach]
	 * @param {() => void} [callback]
	 * @returns {void}
	 */
	function transition_out(block, local, detach, callback) {
		if (block && block.o) {
			if (outroing.has(block)) return;
			outroing.add(block);
			outros.c.push(() => {
				outroing.delete(block);
				if (callback) {
					if (detach) block.d(1);
					callback();
				}
			});
			block.o(local);
		} else if (callback) {
			callback();
		}
	}

	/** @typedef {1} INTRO */
	/** @typedef {0} OUTRO */
	/** @typedef {{ direction: 'in' | 'out' | 'both' }} TransitionOptions */
	/** @typedef {(node: Element, params: any, options: TransitionOptions) => import('../transition/public.js').TransitionConfig} TransitionFn */

	/**
	 * @typedef {Object} Outro
	 * @property {number} r
	 * @property {Function[]} c
	 * @property {Object} p
	 */

	/**
	 * @typedef {Object} PendingProgram
	 * @property {number} start
	 * @property {INTRO|OUTRO} b
	 * @property {Outro} [group]
	 */

	/**
	 * @typedef {Object} Program
	 * @property {number} a
	 * @property {INTRO|OUTRO} b
	 * @property {1|-1} d
	 * @property {number} duration
	 * @property {number} start
	 * @property {number} end
	 * @property {Outro} [group]
	 */

	// general each functions:

	function ensure_array_like(array_like_or_iterator) {
		return array_like_or_iterator?.length !== undefined
			? array_like_or_iterator
			: Array.from(array_like_or_iterator);
	}

	// keyed each functions:

	/** @returns {void} */
	function destroy_block(block, lookup) {
		block.d(1);
		lookup.delete(block.key);
	}

	/** @returns {any[]} */
	function update_keyed_each(
		old_blocks,
		dirty,
		get_key,
		dynamic,
		ctx,
		list,
		lookup,
		node,
		destroy,
		create_each_block,
		next,
		get_context
	) {
		let o = old_blocks.length;
		let n = list.length;
		let i = o;
		const old_indexes = {};
		while (i--) old_indexes[old_blocks[i].key] = i;
		const new_blocks = [];
		const new_lookup = new Map();
		const deltas = new Map();
		const updates = [];
		i = n;
		while (i--) {
			const child_ctx = get_context(ctx, list, i);
			const key = get_key(child_ctx);
			let block = lookup.get(key);
			if (!block) {
				block = create_each_block(key, child_ctx);
				block.c();
			} else {
				// defer updates until all the DOM shuffling is done
				updates.push(() => block.p(child_ctx, dirty));
			}
			new_lookup.set(key, (new_blocks[i] = block));
			if (key in old_indexes) deltas.set(key, Math.abs(i - old_indexes[key]));
		}
		const will_move = new Set();
		const did_move = new Set();
		/** @returns {void} */
		function insert(block) {
			transition_in(block, 1);
			block.m(node, next);
			lookup.set(block.key, block);
			next = block.first;
			n--;
		}
		while (o && n) {
			const new_block = new_blocks[n - 1];
			const old_block = old_blocks[o - 1];
			const new_key = new_block.key;
			const old_key = old_block.key;
			if (new_block === old_block) {
				// do nothing
				next = new_block.first;
				o--;
				n--;
			} else if (!new_lookup.has(old_key)) {
				// remove old block
				destroy(old_block, lookup);
				o--;
			} else if (!lookup.has(new_key) || will_move.has(new_key)) {
				insert(new_block);
			} else if (did_move.has(old_key)) {
				o--;
			} else if (deltas.get(new_key) > deltas.get(old_key)) {
				did_move.add(new_key);
				insert(new_block);
			} else {
				will_move.add(old_key);
				o--;
			}
		}
		while (o--) {
			const old_block = old_blocks[o];
			if (!new_lookup.has(old_block.key)) destroy(old_block, lookup);
		}
		while (n) insert(new_blocks[n - 1]);
		run_all(updates);
		return new_blocks;
	}

	/** @returns {void} */
	function create_component(block) {
		block && block.c();
	}

	/** @returns {void} */
	function mount_component(component, target, anchor) {
		const { fragment, after_update } = component.$$;
		fragment && fragment.m(target, anchor);
		// onMount happens before the initial afterUpdate
		add_render_callback(() => {
			const new_on_destroy = component.$$.on_mount.map(run).filter(is_function);
			// if the component was destroyed immediately
			// it will update the `$$.on_destroy` reference to `null`.
			// the destructured on_destroy may still reference to the old array
			if (component.$$.on_destroy) {
				component.$$.on_destroy.push(...new_on_destroy);
			} else {
				// Edge case - component was destroyed immediately,
				// most likely as a result of a binding initialising
				run_all(new_on_destroy);
			}
			component.$$.on_mount = [];
		});
		after_update.forEach(add_render_callback);
	}

	/** @returns {void} */
	function destroy_component(component, detaching) {
		const $$ = component.$$;
		if ($$.fragment !== null) {
			flush_render_callbacks($$.after_update);
			run_all($$.on_destroy);
			$$.fragment && $$.fragment.d(detaching);
			// TODO null out other refs, including component.$$ (but need to
			// preserve final state?)
			$$.on_destroy = $$.fragment = null;
			$$.ctx = [];
		}
	}

	/** @returns {void} */
	function make_dirty(component, i) {
		if (component.$$.dirty[0] === -1) {
			dirty_components.push(component);
			schedule_update();
			component.$$.dirty.fill(0);
		}
		component.$$.dirty[(i / 31) | 0] |= 1 << i % 31;
	}

	// TODO: Document the other params
	/**
	 * @param {SvelteComponent} component
	 * @param {import('./public.js').ComponentConstructorOptions} options
	 *
	 * @param {import('./utils.js')['not_equal']} not_equal Used to compare props and state values.
	 * @param {(target: Element | ShadowRoot) => void} [append_styles] Function that appends styles to the DOM when the component is first initialised.
	 * This will be the `add_css` function from the compiled component.
	 *
	 * @returns {void}
	 */
	function init(
		component,
		options,
		instance,
		create_fragment,
		not_equal,
		props,
		append_styles = null,
		dirty = [-1]
	) {
		const parent_component = current_component;
		set_current_component(component);
		/** @type {import('./private.js').T$$} */
		const $$ = (component.$$ = {
			fragment: null,
			ctx: [],
			// state
			props,
			update: noop,
			not_equal,
			bound: blank_object(),
			// lifecycle
			on_mount: [],
			on_destroy: [],
			on_disconnect: [],
			before_update: [],
			after_update: [],
			context: new Map(options.context || (parent_component ? parent_component.$$.context : [])),
			// everything else
			callbacks: blank_object(),
			dirty,
			skip_bound: false,
			root: options.target || parent_component.$$.root
		});
		append_styles && append_styles($$.root);
		let ready = false;
		$$.ctx = instance
			? instance(component, options.props || {}, (i, ret, ...rest) => {
					const value = rest.length ? rest[0] : ret;
					if ($$.ctx && not_equal($$.ctx[i], ($$.ctx[i] = value))) {
						if (!$$.skip_bound && $$.bound[i]) $$.bound[i](value);
						if (ready) make_dirty(component, i);
					}
					return ret;
			  })
			: [];
		$$.update();
		ready = true;
		run_all($$.before_update);
		// `false` as a special case of no DOM component
		$$.fragment = create_fragment ? create_fragment($$.ctx) : false;
		if (options.target) {
			if (options.hydrate) {
				// TODO: what is the correct type here?
				// @ts-expect-error
				const nodes = children(options.target);
				$$.fragment && $$.fragment.l(nodes);
				nodes.forEach(detach);
			} else {
				// eslint-disable-next-line @typescript-eslint/no-non-null-assertion
				$$.fragment && $$.fragment.c();
			}
			if (options.intro) transition_in(component.$$.fragment);
			mount_component(component, options.target, options.anchor);
			flush();
		}
		set_current_component(parent_component);
	}

	/**
	 * Base class for Svelte components. Used when dev=false.
	 *
	 * @template {Record<string, any>} [Props=any]
	 * @template {Record<string, any>} [Events=any]
	 */
	class SvelteComponent {
		/**
		 * ### PRIVATE API
		 *
		 * Do not use, may change at any time
		 *
		 * @type {any}
		 */
		$$ = undefined;
		/**
		 * ### PRIVATE API
		 *
		 * Do not use, may change at any time
		 *
		 * @type {any}
		 */
		$$set = undefined;

		/** @returns {void} */
		$destroy() {
			destroy_component(this, 1);
			this.$destroy = noop;
		}

		/**
		 * @template {Extract<keyof Events, string>} K
		 * @param {K} type
		 * @param {((e: Events[K]) => void) | null | undefined} callback
		 * @returns {() => void}
		 */
		$on(type, callback) {
			if (!is_function(callback)) {
				return noop;
			}
			const callbacks = this.$$.callbacks[type] || (this.$$.callbacks[type] = []);
			callbacks.push(callback);
			return () => {
				const index = callbacks.indexOf(callback);
				if (index !== -1) callbacks.splice(index, 1);
			};
		}

		/**
		 * @param {Partial<Props>} props
		 * @returns {void}
		 */
		$set(props) {
			if (this.$$set && !is_empty(props)) {
				this.$$.skip_bound = true;
				this.$$set(props);
				this.$$.skip_bound = false;
			}
		}
	}

	/**
	 * @typedef {Object} CustomElementPropDefinition
	 * @property {string} [attribute]
	 * @property {boolean} [reflect]
	 * @property {'String'|'Boolean'|'Number'|'Array'|'Object'} [type]
	 */

	// generated during release, do not modify

	const PUBLIC_VERSION = '4';

	if (typeof window !== 'undefined')
		// @ts-ignore
		(window.__svelte || (window.__svelte = { v: new Set() })).v.add(PUBLIC_VERSION);

	/* src/components/ContactList.svelte generated by Svelte v4.2.20 */

	function get_each_context(ctx, list, i) {
		const child_ctx = ctx.slice();
		child_ctx[1] = list[i];
		return child_ctx;
	}

	// (13:0) {:else}
	function create_else_block$1(ctx) {
		let div;
		let each_blocks = [];
		let each_1_lookup = new Map();
		let each_value = ensure_array_like(/*contacts*/ ctx[0]);
		const get_key = ctx => /*contact*/ ctx[1].id;

		for (let i = 0; i < each_value.length; i += 1) {
			let child_ctx = get_each_context(ctx, each_value, i);
			let key = get_key(child_ctx);
			each_1_lookup.set(key, each_blocks[i] = create_each_block(key, child_ctx));
		}

		return {
			c() {
				div = element("div");

				for (let i = 0; i < each_blocks.length; i += 1) {
					each_blocks[i].c();
				}

				attr(div, "class", "grid svelte-yqqhyr");
			},
			m(target, anchor) {
				insert(target, div, anchor);

				for (let i = 0; i < each_blocks.length; i += 1) {
					if (each_blocks[i]) {
						each_blocks[i].m(div, null);
					}
				}
			},
			p(ctx, dirty) {
				if (dirty & /*contacts, Boolean, initials*/ 1) {
					each_value = ensure_array_like(/*contacts*/ ctx[0]);
					each_blocks = update_keyed_each(each_blocks, dirty, get_key, 1, ctx, each_value, each_1_lookup, div, destroy_block, create_each_block, null, get_each_context);
				}
			},
			d(detaching) {
				if (detaching) {
					detach(div);
				}

				for (let i = 0; i < each_blocks.length; i += 1) {
					each_blocks[i].d();
				}
			}
		};
	}

	// (11:0) {#if contacts.length === 0}
	function create_if_block$2(ctx) {
		let p;

		return {
			c() {
				p = element("p");
				p.textContent = "No contacts found.";
				attr(p, "class", "empty svelte-yqqhyr");
			},
			m(target, anchor) {
				insert(target, p, anchor);
			},
			p: noop,
			d(detaching) {
				if (detaching) {
					detach(p);
				}
			}
		};
	}

	// (20:10) {:else}
	function create_else_block_1(ctx) {
		let span;
		let t_value = initials(/*contact*/ ctx[1]) + "";
		let t;

		return {
			c() {
				span = element("span");
				t = text(t_value);
				attr(span, "class", "initials svelte-yqqhyr");
			},
			m(target, anchor) {
				insert(target, span, anchor);
				append(span, t);
			},
			p(ctx, dirty) {
				if (dirty & /*contacts*/ 1 && t_value !== (t_value = initials(/*contact*/ ctx[1]) + "")) set_data(t, t_value);
			},
			d(detaching) {
				if (detaching) {
					detach(span);
				}
			}
		};
	}

	// (18:10) {#if contact.avatar_url}
	function create_if_block_4(ctx) {
		let img;
		let img_src_value;
		let img_alt_value;

		return {
			c() {
				img = element("img");
				if (!src_url_equal(img.src, img_src_value = /*contact*/ ctx[1].avatar_url)) attr(img, "src", img_src_value);
				attr(img, "alt", img_alt_value = "" + (/*contact*/ ctx[1].first_name + " " + /*contact*/ ctx[1].last_name));
				attr(img, "class", "svelte-yqqhyr");
			},
			m(target, anchor) {
				insert(target, img, anchor);
			},
			p(ctx, dirty) {
				if (dirty & /*contacts*/ 1 && !src_url_equal(img.src, img_src_value = /*contact*/ ctx[1].avatar_url)) {
					attr(img, "src", img_src_value);
				}

				if (dirty & /*contacts*/ 1 && img_alt_value !== (img_alt_value = "" + (/*contact*/ ctx[1].first_name + " " + /*contact*/ ctx[1].last_name))) {
					attr(img, "alt", img_alt_value);
				}
			},
			d(detaching) {
				if (detaching) {
					detach(img);
				}
			}
		};
	}

	// (27:10) {#if contact.phone}
	function create_if_block_3(ctx) {
		let span;
		let t_value = /*contact*/ ctx[1].phone + "";
		let t;

		return {
			c() {
				span = element("span");
				t = text(t_value);
			},
			m(target, anchor) {
				insert(target, span, anchor);
				append(span, t);
			},
			p(ctx, dirty) {
				if (dirty & /*contacts*/ 1 && t_value !== (t_value = /*contact*/ ctx[1].phone + "")) set_data(t, t_value);
			},
			d(detaching) {
				if (detaching) {
					detach(span);
				}
			}
		};
	}

	// (28:10) {#if contact.company}
	function create_if_block_2$1(ctx) {
		let span;
		let t_value = /*contact*/ ctx[1].company + "";
		let t;

		return {
			c() {
				span = element("span");
				t = text(t_value);
				attr(span, "class", "muted svelte-yqqhyr");
			},
			m(target, anchor) {
				insert(target, span, anchor);
				append(span, t);
			},
			p(ctx, dirty) {
				if (dirty & /*contacts*/ 1 && t_value !== (t_value = /*contact*/ ctx[1].company + "")) set_data(t, t_value);
			},
			d(detaching) {
				if (detaching) {
					detach(span);
				}
			}
		};
	}

	// (29:10) {#if contact.city || contact.state}
	function create_if_block_1$1(ctx) {
		let span;
		let t_value = [/*contact*/ ctx[1].city, /*contact*/ ctx[1].state].filter(Boolean).join(', ') + "";
		let t;

		return {
			c() {
				span = element("span");
				t = text(t_value);
				attr(span, "class", "muted svelte-yqqhyr");
			},
			m(target, anchor) {
				insert(target, span, anchor);
				append(span, t);
			},
			p(ctx, dirty) {
				if (dirty & /*contacts*/ 1 && t_value !== (t_value = [/*contact*/ ctx[1].city, /*contact*/ ctx[1].state].filter(Boolean).join(', ') + "")) set_data(t, t_value);
			},
			d(detaching) {
				if (detaching) {
					detach(span);
				}
			}
		};
	}

	// (15:4) {#each contacts as contact (contact.id)}
	function create_each_block(key_1, ctx) {
		let div2;
		let div0;
		let t0;
		let div1;
		let strong;
		let t1_value = /*contact*/ ctx[1].first_name + "";
		let t1;
		let t2;
		let t3_value = /*contact*/ ctx[1].last_name + "";
		let t3;
		let t4;
		let a;
		let t5_value = /*contact*/ ctx[1].email + "";
		let t5;
		let a_href_value;
		let t6;
		let t7;
		let t8;
		let t9;

		function select_block_type_1(ctx, dirty) {
			if (/*contact*/ ctx[1].avatar_url) return create_if_block_4;
			return create_else_block_1;
		}

		let current_block_type = select_block_type_1(ctx);
		let if_block0 = current_block_type(ctx);
		let if_block1 = /*contact*/ ctx[1].phone && create_if_block_3(ctx);
		let if_block2 = /*contact*/ ctx[1].company && create_if_block_2$1(ctx);
		let if_block3 = (/*contact*/ ctx[1].city || /*contact*/ ctx[1].state) && create_if_block_1$1(ctx);

		return {
			key: key_1,
			first: null,
			c() {
				div2 = element("div");
				div0 = element("div");
				if_block0.c();
				t0 = space();
				div1 = element("div");
				strong = element("strong");
				t1 = text(t1_value);
				t2 = space();
				t3 = text(t3_value);
				t4 = space();
				a = element("a");
				t5 = text(t5_value);
				t6 = space();
				if (if_block1) if_block1.c();
				t7 = space();
				if (if_block2) if_block2.c();
				t8 = space();
				if (if_block3) if_block3.c();
				t9 = space();
				attr(div0, "class", "avatar svelte-yqqhyr");
				attr(strong, "class", "svelte-yqqhyr");
				attr(a, "href", a_href_value = "mailto:" + /*contact*/ ctx[1].email);
				attr(a, "class", "svelte-yqqhyr");
				attr(div1, "class", "info svelte-yqqhyr");
				attr(div2, "class", "card svelte-yqqhyr");
				this.first = div2;
			},
			m(target, anchor) {
				insert(target, div2, anchor);
				append(div2, div0);
				if_block0.m(div0, null);
				append(div2, t0);
				append(div2, div1);
				append(div1, strong);
				append(strong, t1);
				append(strong, t2);
				append(strong, t3);
				append(div1, t4);
				append(div1, a);
				append(a, t5);
				append(div1, t6);
				if (if_block1) if_block1.m(div1, null);
				append(div1, t7);
				if (if_block2) if_block2.m(div1, null);
				append(div1, t8);
				if (if_block3) if_block3.m(div1, null);
				append(div2, t9);
			},
			p(new_ctx, dirty) {
				ctx = new_ctx;

				if (current_block_type === (current_block_type = select_block_type_1(ctx)) && if_block0) {
					if_block0.p(ctx, dirty);
				} else {
					if_block0.d(1);
					if_block0 = current_block_type(ctx);

					if (if_block0) {
						if_block0.c();
						if_block0.m(div0, null);
					}
				}

				if (dirty & /*contacts*/ 1 && t1_value !== (t1_value = /*contact*/ ctx[1].first_name + "")) set_data(t1, t1_value);
				if (dirty & /*contacts*/ 1 && t3_value !== (t3_value = /*contact*/ ctx[1].last_name + "")) set_data(t3, t3_value);
				if (dirty & /*contacts*/ 1 && t5_value !== (t5_value = /*contact*/ ctx[1].email + "")) set_data(t5, t5_value);

				if (dirty & /*contacts*/ 1 && a_href_value !== (a_href_value = "mailto:" + /*contact*/ ctx[1].email)) {
					attr(a, "href", a_href_value);
				}

				if (/*contact*/ ctx[1].phone) {
					if (if_block1) {
						if_block1.p(ctx, dirty);
					} else {
						if_block1 = create_if_block_3(ctx);
						if_block1.c();
						if_block1.m(div1, t7);
					}
				} else if (if_block1) {
					if_block1.d(1);
					if_block1 = null;
				}

				if (/*contact*/ ctx[1].company) {
					if (if_block2) {
						if_block2.p(ctx, dirty);
					} else {
						if_block2 = create_if_block_2$1(ctx);
						if_block2.c();
						if_block2.m(div1, t8);
					}
				} else if (if_block2) {
					if_block2.d(1);
					if_block2 = null;
				}

				if (/*contact*/ ctx[1].city || /*contact*/ ctx[1].state) {
					if (if_block3) {
						if_block3.p(ctx, dirty);
					} else {
						if_block3 = create_if_block_1$1(ctx);
						if_block3.c();
						if_block3.m(div1, null);
					}
				} else if (if_block3) {
					if_block3.d(1);
					if_block3 = null;
				}
			},
			d(detaching) {
				if (detaching) {
					detach(div2);
				}

				if_block0.d();
				if (if_block1) if_block1.d();
				if (if_block2) if_block2.d();
				if (if_block3) if_block3.d();
			}
		};
	}

	function create_fragment$2(ctx) {
		let if_block_anchor;

		function select_block_type(ctx, dirty) {
			if (/*contacts*/ ctx[0].length === 0) return create_if_block$2;
			return create_else_block$1;
		}

		let current_block_type = select_block_type(ctx);
		let if_block = current_block_type(ctx);

		return {
			c() {
				if_block.c();
				if_block_anchor = empty();
			},
			m(target, anchor) {
				if_block.m(target, anchor);
				insert(target, if_block_anchor, anchor);
			},
			p(ctx, [dirty]) {
				if (current_block_type === (current_block_type = select_block_type(ctx)) && if_block) {
					if_block.p(ctx, dirty);
				} else {
					if_block.d(1);
					if_block = current_block_type(ctx);

					if (if_block) {
						if_block.c();
						if_block.m(if_block_anchor.parentNode, if_block_anchor);
					}
				}
			},
			i: noop,
			o: noop,
			d(detaching) {
				if (detaching) {
					detach(if_block_anchor);
				}

				if_block.d(detaching);
			}
		};
	}

	function initials(contact) {
		const f = contact.first_name?.[0] ?? '';
		const l = contact.last_name?.[0] ?? '';
		return (f + l).toUpperCase() || '?';
	}

	function instance$2($$self, $$props, $$invalidate) {
		let { contacts = [] } = $$props;

		$$self.$$set = $$props => {
			if ('contacts' in $$props) $$invalidate(0, contacts = $$props.contacts);
		};

		return [contacts];
	}

	class ContactList extends SvelteComponent {
		constructor(options) {
			super();
			init(this, options, instance$2, create_fragment$2, safe_not_equal, { contacts: 0 });
		}
	}

	/* src/components/SearchBar.svelte generated by Svelte v4.2.20 */

	function create_if_block$1(ctx) {
		let button;
		let mounted;
		let dispose;

		return {
			c() {
				button = element("button");
				button.textContent = "✕";
				attr(button, "class", "clear svelte-13n3uoi");
				attr(button, "aria-label", "Clear");
			},
			m(target, anchor) {
				insert(target, button, anchor);

				if (!mounted) {
					dispose = listen(button, "click", /*handleClear*/ ctx[3]);
					mounted = true;
				}
			},
			p: noop,
			d(detaching) {
				if (detaching) {
					detach(button);
				}

				mounted = false;
				dispose();
			}
		};
	}

	function create_fragment$1(ctx) {
		let div;
		let input;
		let t;
		let mounted;
		let dispose;
		let if_block = /*inputValue*/ ctx[0] && create_if_block$1(ctx);

		return {
			c() {
				div = element("div");
				input = element("input");
				t = space();
				if (if_block) if_block.c();
				attr(input, "type", "search");
				attr(input, "placeholder", "Search contacts…");
				attr(input, "class", "svelte-13n3uoi");
				attr(div, "class", "search-bar svelte-13n3uoi");
			},
			m(target, anchor) {
				insert(target, div, anchor);
				append(div, input);
				set_input_value(input, /*inputValue*/ ctx[0]);
				append(div, t);
				if (if_block) if_block.m(div, null);

				if (!mounted) {
					dispose = [
						listen(input, "input", /*input_input_handler*/ ctx[7]),
						listen(input, "input", /*scheduleSearch*/ ctx[1]),
						listen(input, "keydown", /*handleKeydown*/ ctx[2])
					];

					mounted = true;
				}
			},
			p(ctx, [dirty]) {
				if (dirty & /*inputValue*/ 1 && input.value !== /*inputValue*/ ctx[0]) {
					set_input_value(input, /*inputValue*/ ctx[0]);
				}

				if (/*inputValue*/ ctx[0]) {
					if (if_block) {
						if_block.p(ctx, dirty);
					} else {
						if_block = create_if_block$1(ctx);
						if_block.c();
						if_block.m(div, null);
					}
				} else if (if_block) {
					if_block.d(1);
					if_block = null;
				}
			},
			i: noop,
			o: noop,
			d(detaching) {
				if (detaching) {
					detach(div);
				}

				if (if_block) if_block.d();
				mounted = false;
				run_all(dispose);
			}
		};
	}

	function instance$1($$self, $$props, $$invalidate) {
		let { query = '' } = $$props;
		let { onSearch } = $$props;
		let { onClear } = $$props;
		let inputValue = query;
		let debounceTimer;

		function scheduleSearch() {
			clearTimeout(debounceTimer);

			if (inputValue.trim()) {
				debounceTimer = setTimeout(() => onSearch(inputValue.trim()), 300);
			} else {
				onClear();
			}
		}

		function handleKeydown(e) {
			if (e.key === 'Enter') {
				clearTimeout(debounceTimer);
				const v = inputValue.trim();
				if (v) onSearch(v); else onClear();
			}
		}

		function handleClear() {
			clearTimeout(debounceTimer);
			$$invalidate(0, inputValue = '');
			onClear();
		}

		function input_input_handler() {
			inputValue = this.value;
			$$invalidate(0, inputValue);
		}

		$$self.$$set = $$props => {
			if ('query' in $$props) $$invalidate(4, query = $$props.query);
			if ('onSearch' in $$props) $$invalidate(5, onSearch = $$props.onSearch);
			if ('onClear' in $$props) $$invalidate(6, onClear = $$props.onClear);
		};

		return [
			inputValue,
			scheduleSearch,
			handleKeydown,
			handleClear,
			query,
			onSearch,
			onClear,
			input_input_handler
		];
	}

	class SearchBar extends SvelteComponent {
		constructor(options) {
			super();
			init(this, options, instance$1, create_fragment$1, safe_not_equal, { query: 4, onSearch: 5, onClear: 6 });
		}
	}

	/* src/App.svelte generated by Svelte v4.2.20 */

	function create_else_block(ctx) {
		let contactlist;
		let t;
		let if_block_anchor;
		let current;
		contactlist = new ContactList({ props: { contacts: /*contacts*/ ctx[1] } });
		let if_block = /*mode*/ ctx[0] === 'list' && create_if_block_2(ctx);

		return {
			c() {
				create_component(contactlist.$$.fragment);
				t = space();
				if (if_block) if_block.c();
				if_block_anchor = empty();
			},
			m(target, anchor) {
				mount_component(contactlist, target, anchor);
				insert(target, t, anchor);
				if (if_block) if_block.m(target, anchor);
				insert(target, if_block_anchor, anchor);
				current = true;
			},
			p(ctx, dirty) {
				const contactlist_changes = {};
				if (dirty & /*contacts*/ 2) contactlist_changes.contacts = /*contacts*/ ctx[1];
				contactlist.$set(contactlist_changes);

				if (/*mode*/ ctx[0] === 'list') {
					if (if_block) {
						if_block.p(ctx, dirty);
					} else {
						if_block = create_if_block_2(ctx);
						if_block.c();
						if_block.m(if_block_anchor.parentNode, if_block_anchor);
					}
				} else if (if_block) {
					if_block.d(1);
					if_block = null;
				}
			},
			i(local) {
				if (current) return;
				transition_in(contactlist.$$.fragment, local);
				current = true;
			},
			o(local) {
				transition_out(contactlist.$$.fragment, local);
				current = false;
			},
			d(detaching) {
				if (detaching) {
					detach(t);
					detach(if_block_anchor);
				}

				destroy_component(contactlist, detaching);
				if (if_block) if_block.d(detaching);
			}
		};
	}

	// (90:18) 
	function create_if_block_1(ctx) {
		let div;
		let t0;
		let t1;

		return {
			c() {
				div = element("div");
				t0 = text("Error: ");
				t1 = text(/*error*/ ctx[6]);
				attr(div, "class", "status error svelte-odiagg");
			},
			m(target, anchor) {
				insert(target, div, anchor);
				append(div, t0);
				append(div, t1);
			},
			p(ctx, dirty) {
				if (dirty & /*error*/ 64) set_data(t1, /*error*/ ctx[6]);
			},
			i: noop,
			o: noop,
			d(detaching) {
				if (detaching) {
					detach(div);
				}
			}
		};
	}

	// (88:2) {#if loading}
	function create_if_block(ctx) {
		let div;

		return {
			c() {
				div = element("div");
				div.textContent = "Loading…";
				attr(div, "class", "status svelte-odiagg");
			},
			m(target, anchor) {
				insert(target, div, anchor);
			},
			p: noop,
			i: noop,
			o: noop,
			d(detaching) {
				if (detaching) {
					detach(div);
				}
			}
		};
	}

	// (95:4) {#if mode === 'list'}
	function create_if_block_2(ctx) {
		let nav;
		let button0;
		let t0;
		let button0_disabled_value;
		let t1;
		let span;
		let t2;
		let t3_value = /*page*/ ctx[3] + 1 + "";
		let t3;
		let t4;
		let t5_value = /*totalPages*/ ctx[11]() + "";
		let t5;
		let t6;
		let button1;
		let t7;
		let button1_disabled_value;
		let mounted;
		let dispose;

		return {
			c() {
				nav = element("nav");
				button0 = element("button");
				t0 = text("← Prev");
				t1 = space();
				span = element("span");
				t2 = text("Page ");
				t3 = text(t3_value);
				t4 = text(" of ");
				t5 = text(t5_value);
				t6 = space();
				button1 = element("button");
				t7 = text("Next →");
				button0.disabled = button0_disabled_value = /*page*/ ctx[3] === 0;
				attr(button0, "class", "svelte-odiagg");
				button1.disabled = button1_disabled_value = (/*page*/ ctx[3] + 1) * pageSize >= /*total*/ ctx[2];
				attr(button1, "class", "svelte-odiagg");
				attr(nav, "class", "pagination svelte-odiagg");
			},
			m(target, anchor) {
				insert(target, nav, anchor);
				append(nav, button0);
				append(button0, t0);
				append(nav, t1);
				append(nav, span);
				append(span, t2);
				append(span, t3);
				append(span, t4);
				append(span, t5);
				append(nav, t6);
				append(nav, button1);
				append(button1, t7);

				if (!mounted) {
					dispose = [
						listen(button0, "click", /*prevPage*/ ctx[9]),
						listen(button1, "click", /*nextPage*/ ctx[10])
					];

					mounted = true;
				}
			},
			p(ctx, dirty) {
				if (dirty & /*page*/ 8 && button0_disabled_value !== (button0_disabled_value = /*page*/ ctx[3] === 0)) {
					button0.disabled = button0_disabled_value;
				}

				if (dirty & /*page*/ 8 && t3_value !== (t3_value = /*page*/ ctx[3] + 1 + "")) set_data(t3, t3_value);

				if (dirty & /*page, total*/ 12 && button1_disabled_value !== (button1_disabled_value = (/*page*/ ctx[3] + 1) * pageSize >= /*total*/ ctx[2])) {
					button1.disabled = button1_disabled_value;
				}
			},
			d(detaching) {
				if (detaching) {
					detach(nav);
				}

				mounted = false;
				run_all(dispose);
			}
		};
	}

	function create_fragment(ctx) {
		let main;
		let header;
		let h1;
		let t1;
		let searchbar;
		let t2;
		let current_block_type_index;
		let if_block;
		let current;

		searchbar = new SearchBar({
				props: {
					query: /*query*/ ctx[4],
					onSearch: /*handleSearch*/ ctx[7],
					onClear: /*handleClear*/ ctx[8]
				}
			});

		const if_block_creators = [create_if_block, create_if_block_1, create_else_block];
		const if_blocks = [];

		function select_block_type(ctx, dirty) {
			if (/*loading*/ ctx[5]) return 0;
			if (/*error*/ ctx[6]) return 1;
			return 2;
		}

		current_block_type_index = select_block_type(ctx);
		if_block = if_blocks[current_block_type_index] = if_block_creators[current_block_type_index](ctx);

		return {
			c() {
				main = element("main");
				header = element("header");
				h1 = element("h1");
				h1.textContent = "Address Book";
				t1 = space();
				create_component(searchbar.$$.fragment);
				t2 = space();
				if_block.c();
				attr(h1, "class", "svelte-odiagg");
				attr(header, "class", "svelte-odiagg");
				attr(main, "class", "svelte-odiagg");
			},
			m(target, anchor) {
				insert(target, main, anchor);
				append(main, header);
				append(header, h1);
				append(header, t1);
				mount_component(searchbar, header, null);
				append(main, t2);
				if_blocks[current_block_type_index].m(main, null);
				current = true;
			},
			p(ctx, [dirty]) {
				const searchbar_changes = {};
				if (dirty & /*query*/ 16) searchbar_changes.query = /*query*/ ctx[4];
				searchbar.$set(searchbar_changes);
				let previous_block_index = current_block_type_index;
				current_block_type_index = select_block_type(ctx);

				if (current_block_type_index === previous_block_index) {
					if_blocks[current_block_type_index].p(ctx, dirty);
				} else {
					group_outros();

					transition_out(if_blocks[previous_block_index], 1, 1, () => {
						if_blocks[previous_block_index] = null;
					});

					check_outros();
					if_block = if_blocks[current_block_type_index];

					if (!if_block) {
						if_block = if_blocks[current_block_type_index] = if_block_creators[current_block_type_index](ctx);
						if_block.c();
					} else {
						if_block.p(ctx, dirty);
					}

					transition_in(if_block, 1);
					if_block.m(main, null);
				}
			},
			i(local) {
				if (current) return;
				transition_in(searchbar.$$.fragment, local);
				transition_in(if_block);
				current = true;
			},
			o(local) {
				transition_out(searchbar.$$.fragment, local);
				transition_out(if_block);
				current = false;
			},
			d(detaching) {
				if (detaching) {
					detach(main);
				}

				destroy_component(searchbar);
				if_blocks[current_block_type_index].d();
			}
		};
	}

	let pageSize = 20;

	function instance($$self, $$props, $$invalidate) {
		let mode = 'list';
		let contacts = [];
		let total = 0;
		let page = 0;
		let query = '';
		let loading = false;
		let error = null;

		async function fetchContacts() {
			$$invalidate(5, loading = true);
			$$invalidate(6, error = null);

			try {
				const res = await fetch(`/api/contacts?limit=${pageSize}&offset=${page * pageSize}`);
				if (!res.ok) throw new Error(`HTTP ${res.status}`);
				const json = await res.json();
				$$invalidate(1, contacts = json.data ?? []);
				$$invalidate(2, total = json.meta?.total ?? contacts.length);
			} catch(e) {
				$$invalidate(6, error = e.message);
				$$invalidate(1, contacts = []);
			} finally {
				$$invalidate(5, loading = false);
			}
		}

		async function fetchSearch(q) {
			$$invalidate(5, loading = true);
			$$invalidate(6, error = null);

			try {
				const res = await fetch(`/api/contacts/search?q=${encodeURIComponent(q)}&limit=50`);
				if (!res.ok) throw new Error(`HTTP ${res.status}`);
				const json = await res.json();
				$$invalidate(1, contacts = json.data ?? []);
				$$invalidate(2, total = contacts.length);
			} catch(e) {
				$$invalidate(6, error = e.message);
				$$invalidate(1, contacts = []);
			} finally {
				$$invalidate(5, loading = false);
			}
		}

		function handleSearch(q) {
			$$invalidate(4, query = q);
			$$invalidate(0, mode = 'search');
			$$invalidate(3, page = 0);
			fetchSearch(q);
		}

		function handleClear() {
			$$invalidate(4, query = '');
			$$invalidate(0, mode = 'list');
			$$invalidate(3, page = 0);
			fetchContacts();
		}

		function prevPage() {
			if (page > 0) {
				$$invalidate(3, page -= 1);
				fetchContacts();
			}
		}

		function nextPage() {
			if ((page + 1) * pageSize < total) {
				$$invalidate(3, page += 1);
				fetchContacts();
			}
		}

		const totalPages = () => Math.max(1, Math.ceil(total / pageSize));

		// Initial load
		fetchContacts();

		return [
			mode,
			contacts,
			total,
			page,
			query,
			loading,
			error,
			handleSearch,
			handleClear,
			prevPage,
			nextPage,
			totalPages
		];
	}

	class App extends SvelteComponent {
		constructor(options) {
			super();
			init(this, options, instance, create_fragment, safe_not_equal, {});
		}
	}

	const app = new App({
	  target: document.body,
	});

	return app;

})();
