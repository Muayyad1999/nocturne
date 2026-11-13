import { ChevronRight } from 'lucide-react';
import { useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import { cx } from '@/lib/cx';
import { Icon, type LucideIcon } from '../Icon';
import { useControllableState } from '../overlay/useControllableState';
import styles from './Tree.module.css';

export interface TreeNode {
  id: string;
  label: string;
  icon?: LucideIcon;
  children?: readonly TreeNode[];
  /** Shown at the row's end: a count, a StatusDot. */
  trailing?: ReactNode;
}

export interface TreeProps {
  nodes: readonly TreeNode[];
  /** Names the tree for assistive technology. */
  'aria-label': string;
  /** The selected node's id. */
  selected?: string | null;
  onSelect?: (id: string) => void;
  /** Controlled set of expanded node ids. Leave it out to let the tree manage itself. */
  expanded?: ReadonlySet<string>;
  onExpandedChange?: (expanded: ReadonlySet<string>) => void;
  /** Node ids expanded at first, when the tree manages itself. */
  defaultExpanded?: readonly string[];
  className?: string;
}

interface VisibleNode {
  node: TreeNode;
  level: number;
  parentId: string | null;
  position: number;
  setSize: number;
}

function flatten(
  nodes: readonly TreeNode[],
  expanded: ReadonlySet<string>,
  level = 1,
  parentId: string | null = null,
): VisibleNode[] {
  return nodes.flatMap((node, index) => [
    { node, level, parentId, position: index + 1, setSize: nodes.length },
    ...(node.children && expanded.has(node.id) ? flatten(node.children, expanded, level + 1, node.id) : []),
  ]);
}

const TYPEAHEAD_RESET_MS = 600;

/**
 * A hierarchy: sites and their devices, folders of templates. Arrow keys walk
 * it (right opens, left closes or goes to the parent), Enter selects, and
 * typing jumps to the next matching label.
 */
export function Tree({
  nodes,
  selected = null,
  onSelect,
  expanded: controlledExpanded,
  onExpandedChange,
  defaultExpanded = [],
  className,
  ...rest
}: TreeProps) {
  const [expanded, setExpanded] = useControllableState<ReadonlySet<string>>(
    controlledExpanded,
    new Set(defaultExpanded),
    onExpandedChange,
  );
  const visible = useMemo(() => flatten(nodes, expanded), [nodes, expanded]);
  const [focusedId, setFocusedId] = useState<string | null>(null);
  const treeRef = useRef<HTMLDivElement>(null);
  const typeahead = useRef({ text: '', at: 0 });

  // The one row in the tab order: the focused one, else the selected one, else the first.
  const tabStop =
    visible.find((entry) => entry.node.id === focusedId)?.node.id ??
    visible.find((entry) => entry.node.id === selected)?.node.id ??
    visible[0]?.node.id;

  const focus = (id: string | undefined) => {
    if (id === undefined) return;
    setFocusedId(id);
    treeRef.current?.querySelector<HTMLElement>(`[data-node="${CSS.escape(id)}"]`)?.focus();
  };

  const setOpen = (id: string, open: boolean) => {
    const next = new Set(expanded);
    if (open) next.add(id);
    else next.delete(id);
    setExpanded(next);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const index = visible.findIndex((entry) => entry.node.id === tabStop);
    const current = visible[index];
    if (!current) return;
    const { node, parentId } = current;
    const isParent = Boolean(node.children?.length);
    const isOpen = expanded.has(node.id);

    switch (event.key) {
      case 'ArrowDown':
        focus(visible[index + 1]?.node.id);
        break;
      case 'ArrowUp':
        focus(visible[index - 1]?.node.id);
        break;
      case 'ArrowRight':
        if (isParent && !isOpen) setOpen(node.id, true);
        else if (isParent) focus(visible[index + 1]?.node.id);
        break;
      case 'ArrowLeft':
        if (isParent && isOpen) setOpen(node.id, false);
        else if (parentId !== null) focus(parentId);
        break;
      case 'Home':
        focus(visible[0]?.node.id);
        break;
      case 'End':
        focus(visible.at(-1)?.node.id);
        break;
      case 'Enter':
      case ' ':
        onSelect?.(node.id);
        break;
      default: {
        if (event.key.length !== 1 || event.ctrlKey || event.metaKey || event.altKey) return;
        const now = performance.now();
        const state = typeahead.current;
        state.text = now - state.at > TYPEAHEAD_RESET_MS ? event.key : state.text + event.key;
        state.at = now;
        const query = state.text.toLowerCase();
        const rotated = [...visible.slice(index + 1), ...visible.slice(0, index + 1)];
        focus(rotated.find((entry) => entry.node.label.toLowerCase().startsWith(query))?.node.id);
      }
    }
    event.preventDefault();
  };

  return (
    <div
      ref={treeRef}
      role="tree"
      aria-label={rest['aria-label']}
      className={cx(styles.tree, className)}
      onKeyDown={onKeyDown}
    >
      {visible.map(({ node, level, position, setSize }) => {
        const isParent = Boolean(node.children?.length);
        const isOpen = expanded.has(node.id);
        return (
          <div
            key={node.id}
            role="treeitem"
            data-node={node.id}
            aria-level={level}
            aria-posinset={position}
            aria-setsize={setSize}
            aria-expanded={isParent ? isOpen : undefined}
            aria-selected={node.id === selected}
            tabIndex={node.id === tabStop ? 0 : -1}
            className={styles.row}
            style={{ paddingLeft: `calc(var(--sp-8) + ${level - 1} * var(--sp-16))` }}
            onClick={() => {
              setFocusedId(node.id);
              onSelect?.(node.id);
            }}
            onDoubleClick={() => {
              if (isParent) setOpen(node.id, !isOpen);
            }}
            onFocus={() => setFocusedId(node.id)}
          >
            {/* The chevron is a pointer target only: the keyboard uses the arrow keys. */}
            <span
              className={styles.chevron}
              data-open={isOpen || undefined}
              aria-hidden="true"
              onClick={(event) => {
                if (!isParent) return;
                event.stopPropagation();
                setOpen(node.id, !isOpen);
              }}
            >
              {isParent && <Icon icon={ChevronRight} size={14} />}
            </span>
            {node.icon && <Icon icon={node.icon} size={14} className={styles.icon} />}
            <span className={styles.label}>{node.label}</span>
            {node.trailing && <span className={styles.trailing}>{node.trailing}</span>}
          </div>
        );
      })}
    </div>
  );
}
