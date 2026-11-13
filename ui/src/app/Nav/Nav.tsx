import { PanelLeftClose, PanelLeftOpen, Sparkles } from 'lucide-react';
import { NavLink } from 'react-router';
import { Badge } from '@/design/Badge';
import { Icon } from '@/design/Icon';
import { Kbd } from '@/design/Kbd';
import { LogoMark } from '@/design/LogoMark';
import { Tooltip } from '@/design/Tooltip';
import { formatCompact, formatCount } from '@/lib/format';
import { DESTINATIONS, NAV_GROUPS, destination, type Destination } from '../destinations';
import { useLayout } from '../layout';
import { useNavBadges, type NavBadge } from '../navBadges';
import styles from './Nav.module.css';

function CountBadge({ badge }: { badge: NavBadge }) {
  return (
    <Badge tone={badge.tone ?? 'accent'} className={styles.badge}>
      <span aria-hidden="true">{formatCompact(badge.count)}</span>
      <span className="visually-hidden">
        , {formatCount(badge.count)} {badge.label}
      </span>
    </Badge>
  );
}

function NavItem({ item, collapsed }: { item: Destination; collapsed: boolean }) {
  const badge = useNavBadges((state) => state.badges[item.id]);
  return (
    <li>
      {/* In the collapsed rail the label moves into a tooltip. */}
      <Tooltip
        content={badge ? `${item.label} · ${formatCount(badge.count)} ${badge.label}` : item.label}
        placement="right"
        disabled={!collapsed}
        describe={false}
      >
        <NavLink to={item.path} end={item.path === '/'} className={styles.item}>
          <Icon icon={item.icon} />
          <span className={styles.label}>{item.label}</span>
          {badge && <CountBadge badge={badge} />}
        </NavLink>
      </Tooltip>
    </li>
  );
}

/** The nav rail: brand, grouped destinations, then assistant, settings and the collapse control. */
export function Nav() {
  const collapsed = useLayout((state) => state.navCollapsed);
  const toggleNav = useLayout((state) => state.toggleNav);
  const dockOpen = useLayout((state) => state.dockOpen);
  const toggleDock = useLayout((state) => state.toggleDock);

  return (
    <nav className={styles.nav} aria-label="Primary" data-collapsed={collapsed || undefined}>
      <div className={styles.brand}>
        <LogoMark size={22} className={styles.mark} />
        <span className={styles.wordmark}>Nocturne</span>
      </div>

      <div className={styles.groups}>
        {NAV_GROUPS.map((group) => (
          <section key={group.id} className={styles.group} aria-labelledby={`nav-group-${group.id}`}>
            <h2 id={`nav-group-${group.id}`} className={styles.groupLabel}>
              {group.label}
            </h2>
            <ul>
              {DESTINATIONS.filter((item) => item.group === group.id).map((item) => (
                <NavItem key={item.id} item={item} collapsed={collapsed} />
              ))}
            </ul>
          </section>
        ))}
      </div>

      <ul className={styles.footer}>
        <li>
          <Tooltip
            content={
              <>
                Assistant <Kbd shortcut="mod+j" />
              </>
            }
            placement="right"
            disabled={!collapsed}
            describe={false}
          >
            <button
              type="button"
              className={styles.item}
              data-ai
              aria-pressed={dockOpen}
              onClick={toggleDock}
            >
              <Icon icon={Sparkles} />
              <span className={styles.label}>Assistant</span>
              <Kbd shortcut="mod+j" className={styles.hint} />
            </button>
          </Tooltip>
        </li>
        <NavItem item={destination('settings')} collapsed={collapsed} />
        <li>
          <Tooltip
            content={
              <>
                Expand navigation <Kbd shortcut="mod+b" />
              </>
            }
            placement="right"
            disabled={!collapsed}
            describe={false}
          >
            <button type="button" className={styles.item} aria-expanded={!collapsed} onClick={toggleNav}>
              <Icon icon={collapsed ? PanelLeftOpen : PanelLeftClose} />
              <span className={styles.label}>{collapsed ? 'Expand navigation' : 'Collapse'}</span>
              <Kbd shortcut="mod+b" className={styles.hint} />
            </button>
          </Tooltip>
        </li>
      </ul>
    </nav>
  );
}
