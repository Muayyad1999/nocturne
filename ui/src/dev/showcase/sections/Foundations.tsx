import { LogoMark } from '@/design/LogoMark';
import { Example, Section } from '../parts';
import styles from '../Showcase.module.css';

const SURFACES = [
  ['--bg-0', 'app, terminal'],
  ['--bg-1', 'panels, nav, header'],
  ['--bg-2', 'cards, inputs'],
  ['--bg-3', 'hover'],
  ['--bg-4', 'active, selected'],
  ['--line', 'hairlines'],
  ['--line-strong', 'input borders'],
];
const TEXT = [
  ['--text-1', 'primary'],
  ['--text-2', 'secondary'],
  ['--text-3', 'muted, units'],
  ['--accent', 'actions, focus, links'],
  ['--accent-strong', 'accent hover'],
  ['--accent-soft', 'selected fill'],
  ['--ai', 'assistant'],
  ['--ai-soft', 'assistant fill'],
];
const STATUS = [
  ['--ok', 'up, healthy'],
  ['--warn', 'degraded'],
  ['--crit', 'down, destructive'],
  ['--info', 'informational'],
  ['--idle', 'unknown, off'],
  ['--maint', 'maintenance, lab'],
  ['--ok-soft', 'soft fills…'],
  ['--crit-soft', '…one per status'],
];
const SERIES = Array.from({ length: 8 }, (_, index) => [`--series-${index + 1}`, `chart slot ${index + 1}`]);

const TYPE_SCALE = [
  ['--fs-28', '28 / 600', 'Stat values'],
  ['--fs-20', '20 / 600', 'Large headings'],
  ['--fs-16', '16 / 600', 'Section and dialog titles'],
  ['--fs-14', '14 / 600', 'Page title, panel headings'],
  ['--fs-13', '13 / 400', 'Body: the base size of the interface'],
  ['--fs-12', '12 / 500', 'Labels, table headers, secondary text'],
  ['--fs-11', '11 / 500', 'Badges, status bar, captions'],
] as const;

const SPACE = [2, 4, 6, 8, 12, 16, 20, 24, 32, 48];
const RADII = [
  ['--radius-badge', '4 badges'],
  ['--radius-control', '6 controls'],
  ['--radius-card', '10 cards'],
  ['--radius-dialog', '14 dialogs'],
];

function Swatches({ tokens }: { tokens: string[][] }) {
  return (
    <div className={styles.swatches}>
      {tokens.map(([name, use]) => (
        <div key={name} className={styles.swatch}>
          <span className={styles.chip} style={{ background: `var(${name})` }} />
          <div className="truncate">
            <div className={styles.swatchName}>{name}</div>
            <div className={styles.swatchUse}>{use}</div>
          </div>
        </div>
      ))}
    </div>
  );
}

export function Foundations() {
  return (
    <>
      <Section
        title="Brand"
        description="A crescent drawn as a small constellation of nodes and links: the topology of a network reads as a star map. The mark takes its colour from the text around it."
      >
        <Example title="LogoMark" span={2}>
          <div className={styles.logoRow}>
            <LogoMark size={16} />
            <LogoMark size={20} />
            <LogoMark size={24} title="Nocturne" />
            <LogoMark size={32} />
            <LogoMark size={48} />
            <LogoMark size={72} />
          </div>
        </Example>
        <Example title="With the wordmark">
          <span className={styles.wordmark}>
            <LogoMark size={24} />
            Nocturne
          </span>
        </Example>
      </Section>

      <Section
        title="Colour"
        description="Deep night surfaces, moonlight for emphasis, and colour reserved for meaning. Every value is a token; both themes pass WCAG AA for text."
      >
        <Example title="Surfaces and lines" span={3}>
          <Swatches tokens={SURFACES} />
        </Example>
        <Example title="Text, accent and assistant" span={3}>
          <Swatches tokens={TEXT} />
        </Example>
        <Example title="Status: always with a shape or a word" span={3}>
          <Swatches tokens={STATUS} />
        </Example>
        <Example title="Chart series: identity only, fixed order" span={3}>
          <Swatches tokens={SERIES} />
        </Example>
      </Section>

      <Section title="Type" description="Inter for the interface; JetBrains Mono for anything a device said or will be told.">
        <Example title="Scale" layout="column" span={2}>
          {TYPE_SCALE.map(([token, meta, sample]) => (
            <div key={token} className={styles.typeRow}>
              <span className={styles.typeMeta}>{meta}</span>
              <span style={{ fontSize: `var(${token})`, fontWeight: meta.endsWith('400') ? 400 : meta.endsWith('500') ? 500 : 600 }}>
                {sample}
              </span>
            </div>
          ))}
        </Example>
        <Example title="Monospace and tabular figures" layout="column">
          <span className="mono">interface GigabitEthernet1/0/24</span>
          <span className="mono">192.0.2.117/24 · 2001:db8::1</span>
          <span className="mono">0O 1lI 5S 8B {'{}[]()'} =&gt; !=</span>
          <span className="tnum">1 204 111 · 98.4% · 12 345</span>
        </Example>
      </Section>

      <Section title="Space and radius">
        <Example title="Space scale (px)" span={2}>
          <div className={styles.scale}>
            {SPACE.map((size) => (
              <div key={size} className={styles.scaleItem}>
                <span className={styles.spaceBar} style={{ height: size }} />
                {size}
              </div>
            ))}
          </div>
        </Example>
        <Example title="Radii">
          <div className={styles.scale}>
            {RADII.map(([token, label]) => (
              <div key={token} className={styles.scaleItem}>
                <span className={styles.radiusBox} style={{ borderRadius: `var(${token})` }} />
                {label}
              </div>
            ))}
          </div>
        </Example>
      </Section>
    </>
  );
}
