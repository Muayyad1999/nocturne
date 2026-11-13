import {
  ChevronDown,
  Copy,
  Download,
  Funnel,
  Pencil,
  Plus,
  RefreshCw,
  Sparkles,
  SquareTerminal,
  Trash,
} from 'lucide-react';
import { useState } from 'react';
import { Badge } from '@/design/Badge';
import { Button, IconButton } from '@/design/Button';
import { ContextMenu } from '@/design/ContextMenu';
import { DropdownMenu, type MenuEntry } from '@/design/DropdownMenu';
import { Kbd } from '@/design/Kbd';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { TabPanel, Tabs } from '@/design/Tabs';
import { toast } from '@/design/Toast';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { Example, Section } from '../parts';
import styles from '../Showcase.module.css';

const VARIANTS = [
  ['primary', 'Primary'],
  ['secondary', 'Secondary'],
  ['ghost', 'Ghost'],
  ['danger', 'Danger'],
  ['ai', 'AI'],
] as const;

const DEVICE_ACTIONS: MenuEntry[] = [
  { type: 'label', label: 'core-sw-01' },
  { label: 'Open terminal', icon: SquareTerminal, shortcut: 'mod+enter', onSelect: () => toast.info('Open terminal chosen') },
  { label: 'Copy address', icon: Copy, onSelect: () => toast.info('Copy address chosen') },
  { label: 'Rename', icon: Pencil, disabled: true, onSelect: () => undefined },
  { type: 'separator' },
  { label: 'Remove device', icon: Trash, tone: 'danger', onSelect: () => toast.info('Remove device chosen') },
];

export function Actions() {
  const [tab, setTab] = useState('interfaces');
  const [query, setQuery] = useState('');
  const [site, setSite] = useState('all');
  const [columns, setColumns] = useState({ site: true, platform: true, uptime: false });

  return (
    <>
      <Section
        title="Button"
        description="One primary action per screen; everything else is quiet. Danger is for destructive or device-touching actions, ai for anything the assistant does."
      >
        <Example title="Variants" span={2}>
          {VARIANTS.map(([variant, label]) => (
            <Button key={variant} variant={variant}>
              {label}
            </Button>
          ))}
        </Example>
        <Example title="Small">
          {VARIANTS.map(([variant, label]) => (
            <Button key={variant} variant={variant} size="sm">
              {label}
            </Button>
          ))}
        </Example>
        <Example title="With icons">
          <Button variant="primary" icon={Plus}>
            Add device
          </Button>
          <Button icon={Download}>Export</Button>
          <Button trailingIcon={ChevronDown}>Last 6 hours</Button>
          <Button variant="ai" icon={Sparkles}>
            Explain
          </Button>
        </Example>
        <Example title="Loading: keeps its width and focus" span={2}>
          {VARIANTS.map(([variant]) => (
            <Button key={variant} variant={variant} loading>
              Saving
            </Button>
          ))}
        </Example>
        <Example title="Disabled" span={3}>
          {VARIANTS.map(([variant, label]) => (
            <Button key={variant} variant={variant} disabled>
              {label}
            </Button>
          ))}
        </Example>
        <Example title="Icon only: label required, shown as a tooltip">
          <IconButton icon={RefreshCw} aria-label="Refresh" />
          <IconButton icon={Funnel} aria-label="Filter" variant="secondary" />
          <IconButton icon={Plus} aria-label="Add device" variant="primary" />
          <IconButton icon={Trash} aria-label="Delete" variant="danger" />
          <IconButton icon={Sparkles} aria-label="Ask the assistant" variant="ai" />
          <IconButton icon={RefreshCw} aria-label="Refresh" size="sm" />
          <IconButton icon={RefreshCw} aria-label="Refreshing" loading />
          <IconButton icon={RefreshCw} aria-label="Refresh" disabled />
        </Example>
      </Section>

      <Section title="Toolbar" description="The row at the top of a page: filters on the left, the primary action on the right.">
        <Example title="Page toolbar" span={3} layout="fill">
          <Toolbar>
            <div style={{ width: 240 }}>
              <SearchInput aria-label="Search devices" value={query} onChange={setQuery} />
            </div>
            <div style={{ width: 160 }}>
              <Select
                aria-label="Site"
                value={site}
                onChange={(event) => setSite(event.target.value)}
                options={[
                  { value: 'all', label: 'All sites' },
                  { value: 'dc1', label: 'Amman DC1' },
                  { value: 'dc2', label: 'Amman DC2' },
                ]}
              />
            </div>
            <IconButton icon={RefreshCw} aria-label="Refresh" />
            <ToolbarSpacer />
            <Button icon={Download}>Export</Button>
            <Button variant="primary" icon={Plus}>
              Add device
            </Button>
          </Toolbar>
        </Example>
      </Section>

      <Section title="Tabs" description="Arrow keys move and select; Home and End jump to the ends.">
        <Example title="With icon, badge and a disabled tab" span={2} layout="column">
          <Tabs
            id="showcase-tabs"
            aria-label="Device sections"
            value={tab}
            onChange={setTab}
            tabs={[
              { id: 'interfaces', label: 'Interfaces', badge: <Badge>48</Badge> },
              { id: 'neighbours', label: 'Neighbours' },
              { id: 'config', label: 'Config', icon: Pencil },
              { id: 'alerts', label: 'Alerts', badge: <Badge tone="crit">2</Badge> },
              { id: 'history', label: 'History', disabled: true },
            ]}
          />
          <TabPanel tabs="showcase-tabs" value={tab}>
            <p className={styles.note}>Panel for “{tab}”.</p>
          </TabPanel>
        </Example>
        <Example title="Kbd">
          <Kbd shortcut="mod+k" />
          <Kbd shortcut="mod+shift+p" />
          <Kbd shortcut="alt+up" />
          <Kbd>Esc</Kbd>
          <Kbd>?</Kbd>
        </Example>
      </Section>

      <Section
        title="Menus"
        description="Arrow keys, Home and End, type-ahead, Enter to choose, Escape to close. A context menu is a shortcut: the same actions are always reachable another way."
      >
        <Example title="DropdownMenu">
          <DropdownMenu
            aria-label="Device actions"
            items={DEVICE_ACTIONS}
            trigger={<Button trailingIcon={ChevronDown}>Actions</Button>}
          />
        </Example>
        <Example title="Toggles stay open">
          <DropdownMenu
            aria-label="Columns"
            trigger={<Button trailingIcon={ChevronDown}>Columns</Button>}
            items={(['site', 'platform', 'uptime'] as const).map((key) => ({
              label: key[0]?.toUpperCase() + key.slice(1),
              checked: columns[key],
              onSelect: () => setColumns({ ...columns, [key]: !columns[key] }),
            }))}
          />
        </Example>
        <Example title="ContextMenu">
          <ContextMenu aria-label="Device actions" items={DEVICE_ACTIONS}>
            <div className={styles.dropzone} tabIndex={0}>
              Right-click, or focus and press Shift+F10
            </div>
          </ContextMenu>
        </Example>
      </Section>
    </>
  );
}
