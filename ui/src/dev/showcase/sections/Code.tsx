import { CodeBlock } from '@/design/CodeBlock';
import { DiffView } from '@/design/DiffView';
import { Example, Section } from '../parts';
import { SAMPLE_CONFIG_AFTER, SAMPLE_CONFIG_BEFORE, SAMPLE_OUTPUT } from '../sample';

export function Code() {
  return (
    <>
      <Section title="CodeBlock" description="Text as a device said it or will be told: monospace, selectable, copied exactly.">
        <Example title="Device output with a title" span={2} layout="fill">
          <CodeBlock title="core-sw-01" code={SAMPLE_OUTPUT} />
        </Example>
        <Example title="A command" layout="fill">
          <CodeBlock code="show running-config | section interface" />
        </Example>
        <Example title="Line numbers and a capped height" span={2} layout="fill">
          <CodeBlock title="running-config" code={SAMPLE_CONFIG_BEFORE} lineNumbers maxHeight={180} />
        </Example>
        <Example title="Wrapped" layout="fill">
          <CodeBlock
            wrap
            copyable={false}
            code="%LINEPROTO-5-UPDOWN: Line protocol on Interface GigabitEthernet1/0/3, changed state to down (administratively shut by admin on vty0 from 198.51.100.24)"
          />
        </Example>
      </Section>

      <Section
        title="DiffView"
        description="A unified diff with − and + in the gutter (never colour alone), line numbers for both sides, and unchanged stretches folded until asked for."
      >
        <Example title="A planned change" span={2} layout="fill">
          <DiffView title="core-sw-01 running-config" before={SAMPLE_CONFIG_BEFORE} after={SAMPLE_CONFIG_AFTER} />
        </Example>
        <Example title="No differences" layout="fill">
          <DiffView title="dist-sw-07 running-config" before={SAMPLE_CONFIG_BEFORE} after={SAMPLE_CONFIG_BEFORE} />
        </Example>
      </Section>
    </>
  );
}
