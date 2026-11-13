import { useMemo, useState } from 'react';
import { Button } from '@/design/Button';
import { Card } from '@/design/Card';
import { TimeSeriesChart } from '@/design/TimeSeriesChart';
import { formatCount, formatPercent, formatRate } from '@/lib/format';
import { Example, Section } from '../parts';
import { sampleTraffic, seeded } from '../sample';

export function Charts() {
  const traffic = useMemo(() => sampleTraffic(), []);
  const [loading, setLoading] = useState(false);

  // Four interfaces of one device: the same unit, so they share one axis.
  const utilisation = useMemo(() => {
    const random = seeded(3);
    const timestamps = traffic.timestamps.filter((_, index) => index % 5 === 0);
    const make = (base: number, swing: number) =>
      timestamps.map((_, index) => Math.max(0, Math.min(100, base + Math.sin(index / 6) * swing + random() * 6)));
    return {
      timestamps,
      series: [
        { id: 'gi1', label: 'Gi1/0/1', values: make(62, 14) },
        { id: 'gi2', label: 'Gi1/0/2', values: make(38, 9) },
        { id: 'gi3', label: 'Gi1/0/3', values: make(17, 5) },
      ],
    };
  }, [traffic.timestamps]);

  return (
    <>
      <Section
        title="TimeSeriesChart"
        description="One y-axis, hairline grid, a crosshair that finds the time and a tooltip that lists every series. Colour follows the series, a legend names them, and every chart has a table view. Arrow keys move the crosshair."
      >
        <Example title="Two series: legend, direct labels, a gap where samples are missing" span={3} layout="fill">
          <Card>
            <TimeSeriesChart
              title="Uplink traffic"
              subtitle="core-sw-01 · Te1/1/1 · last 6 hours"
              timestamps={traffic.timestamps}
              series={[
                { id: 'in', label: 'Inbound', values: traffic.inbound },
                { id: 'out', label: 'Outbound', values: traffic.outbound },
              ]}
              formatValue={formatRate}
              loading={loading}
            />
          </Card>
          <div style={{ marginTop: 12 }}>
            <Button size="sm" onClick={() => setLoading(!loading)}>
              {loading ? 'Finish refetch' : 'Simulate a refetch: the frame is held, dimmed'}
            </Button>
          </div>
        </Example>
        <Example title="One series: no legend, the title names it" span={2} layout="fill">
          <Card>
            <TimeSeriesChart
              title="Inbound traffic"
              subtitle="Te1/1/1"
              timestamps={traffic.timestamps}
              series={[{ id: 'in', label: 'Inbound', values: traffic.inbound }]}
              formatValue={formatRate}
              height={180}
            />
          </Card>
        </Example>
        <Example title="A series that is a state wears a status colour" layout="fill">
          <Card>
            <TimeSeriesChart
              title="Input errors"
              subtitle="per minute"
              timestamps={traffic.timestamps}
              series={[{ id: 'errors', label: 'Errors', values: traffic.errors, tone: 'crit' }]}
              formatValue={formatCount}
              height={180}
            />
          </Card>
        </Example>
        <Example title="Fixed range" span={2} layout="fill">
          <Card>
            <TimeSeriesChart
              title="Interface utilisation"
              subtitle="core-sw-01"
              timestamps={utilisation.timestamps}
              series={utilisation.series}
              formatValue={formatPercent}
              min={0}
              max={100}
              height={180}
            />
          </Card>
        </Example>
        <Example title="No data" layout="fill">
          <Card>
            <TimeSeriesChart title="Latency" subtitle="edge-fw-01" timestamps={[]} series={[]} height={180} />
          </Card>
        </Example>
      </Section>
    </>
  );
}
