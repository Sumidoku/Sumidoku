import { Children, Fragment, isValidElement, useEffect, useId, useRef, useState } from 'react';
import type {
	ButtonHTMLAttributes,
	KeyboardEvent,
	MouseEvent,
	OptionHTMLAttributes,
	ReactElement,
	ReactNode
} from 'react';
import { createPortal } from 'react-dom';
import { Check, ChevronDown } from 'lucide-react';
import { moveIndex, selectedIndex, typeaheadIndex } from './selectModel';
import type { SelectOption } from './selectModel';
import { useAnchoredMenu } from './useAnchoredMenu';

const TYPEAHEAD_RESET_MS = 500;

export interface SelectProps
	extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'children' | 'onChange' | 'value'> {
	/** `<option>` elements, as for a native select */
	children: ReactNode;
	value: string | number;
	onChange: (value: string) => void;
}

function collectOptions(children: ReactNode, options: SelectOption[]): SelectOption[] {
	for (const child of Children.toArray(children)) {
		if (!isValidElement(child)) continue;
		const { props, type } = child as ReactElement<OptionHTMLAttributes<HTMLOptionElement>>;
		if (type === Fragment) collectOptions(props.children, options);
		else if (type === 'option') {
			const label = Children.toArray(props.children).join('');
			options.push({ value: String(props.value ?? label), label });
		}
	}
	return options;
}

// The portal keeps React's bubbling, so a pick would otherwise reach the reader's tap zones
function stopBubbling(e: MouseEvent) {
	e.stopPropagation();
}

/** Single-select dropdown drawn by the app, so its open list matches the filter menus */
export function Select({ children, className, value, onChange, 'aria-label': ariaLabel, ...rest }: SelectProps) {
	const options = collectOptions(children, []);
	const selected = selectedIndex(options, String(value));
	const { open, style, anchorRef, menuRef, show, close } = useAnchoredMenu<HTMLButtonElement>();
	const [active, setActive] = useState(selected);
	const typed = useRef({ query: '', at: 0 });
	const listId = useId();

	useEffect(() => {
		if (open) menuRef.current?.children[active]?.scrollIntoView({ block: 'nearest' });
	}, [open, active, menuRef]);

	function openMenu(index = selected) {
		setActive(index);
		show();
		// Safari does not focus a clicked button, and keyboard handling lives on it;
		// a focus scroll would dismiss the menu just opened
		anchorRef.current?.focus({ preventScroll: true });
	}

	function pick(index: number) {
		close();
		if (index !== selected) onChange(options[index].value);
	}

	function typeahead(char: string) {
		const now = Date.now();
		const query = now - typed.current.at < TYPEAHEAD_RESET_MS ? typed.current.query + char : char;
		typed.current = { query, at: now };
		const index = typeaheadIndex(options, query, open ? active : selected);
		if (index === -1) return;
		if (open) setActive(index);
		else openMenu(index);
	}

	function onKeyDown(e: KeyboardEvent<HTMLButtonElement>) {
		if (e.ctrlKey || e.altKey || e.metaKey) return;
		const composing = Date.now() - typed.current.at < TYPEAHEAD_RESET_MS;
		if (e.key.length === 1 && e.key !== ' ') typeahead(e.key);
		else if (e.key === ' ' && composing) typeahead(e.key);
		else if (e.key === 'Tab') {
			close();
			return;
		} else if (!open) {
			if (!['ArrowDown', 'ArrowUp', 'Enter', ' '].includes(e.key)) return;
			openMenu();
		} else if (e.key === 'Escape') close();
		else if (e.key === 'Enter' || e.key === ' ') pick(active);
		else {
			const next = moveIndex(e.key, active, options.length);
			if (next === null) return;
			setActive(next);
		}
		// Past here the key was ours: keep it from the browser and from Modal's Escape listener
		e.preventDefault();
		e.stopPropagation();
	}

	return (
		<span className={className ? `select ${className}` : 'select'}>
			<button
				{...rest}
				ref={anchorRef}
				type="button"
				className="select__field"
				role="combobox"
				aria-label={ariaLabel}
				aria-haspopup="listbox"
				aria-expanded={open}
				aria-controls={open ? listId : undefined}
				aria-activedescendant={open ? `${listId}-${active}` : undefined}
				onClick={() => (open ? close() : openMenu())}
				onKeyDown={onKeyDown}
			>
				<span className="select__value">{options[selected]?.label}</span>
				{/* Every label sits in the same grid cell so the width holds across values, as a native select's does */}
				{options.map((option, index) => (
					<span key={index} className="select__sizer" aria-hidden="true">
						{option.label}
					</span>
				))}
			</button>
			<span className="select__chevron">
				<ChevronDown size={14} />
			</span>
			{open &&
				createPortal(
					<div
						ref={menuRef}
						id={listId}
						role="listbox"
						aria-label={ariaLabel}
						className="chip-dropdown__menu select__menu"
						style={style}
						// Focus stays on the trigger, which owns the keyboard
						onMouseDown={(e) => e.preventDefault()}
						onClick={stopBubbling}
						onPointerDown={stopBubbling}
						onPointerUp={stopBubbling}
					>
						{options.map((option, index) => (
							<div
								key={index}
								id={`${listId}-${index}`}
								role="option"
								aria-selected={index === selected}
								className={
									index === active
										? 'chip-dropdown__item chip-dropdown__item--active'
										: 'chip-dropdown__item'
								}
								onClick={() => pick(index)}
								onMouseEnter={() => setActive(index)}
							>
								<span className="select__option-label">{option.label}</span>
								<span className="chip-dropdown__state" aria-hidden="true">
									{index === selected && <Check size={13} />}
								</span>
							</div>
						))}
					</div>,
					document.body
				)}
		</span>
	);
}
