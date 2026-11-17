/**
 * What is for sale, and what it costs you: the tariffs and the links.
 *
 * A **tariff** is a named plan — megabits, a price per megabit or a flat
 * figure, a currency and a period. Changing one does not change what anybody
 * already on it pays: a subscription copies the figures that were agreed, and
 * the save says how many running subscriptions it therefore left alone.
 *
 * A **link** is an upstream or a tower's feed: what it can carry, and what a
 * megabit on it costs wholesale. It is what makes oversubscription and margin
 * possible — a link's `tower` is the label the customers behind it carry, and
 * its device and interface are what lets a measured figure sit beside the
 * sold one.
 */

import { Cable, Package, Plus } from 'lucide-react';
import { useState } from 'react';
import type { Link as IspLink, TariffRow } from '@/api/types/isp';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Panel } from '@/design/Panel';
import { Table, type TableColumn } from '@/design/Table';
import { toast } from '@/design/Toast';
import { formatCount, plural } from '@/lib/format';
import { useDeleteLink, useDeleteTariff, useDeviceNames, useLinks, useTariffs } from './data';
import { Amount, Words } from './Figures';
import { LinkDialog } from './LinkDialog';
import { PERIOD_WORDS, UNIT_WORDS } from './records';
import { rateText } from './coverage';
import { TariffDialog } from './TariffDialog';
import styles from './isp.module.css';

export function Plans() {
  const tariffs = useTariffs();
  const links = useLinks();
  const devices = useDeviceNames();
  const deleteTariff = useDeleteTariff();
  const deleteLink = useDeleteLink();
  const [tariffDialog, setTariffDialog] = useState<{ open: boolean; tariff: TariffRow | null }>({
    open: false,
    tariff: null,
  });
  const [linkDialog, setLinkDialog] = useState<{ open: boolean; link: IspLink | null }>({
    open: false,
    link: null,
  });

  const deviceName = (id: string | null) =>
    id === null ? null : (devices.find((device) => device.id === id)?.name ?? id);

  const tariffColumns: readonly TableColumn<TariffRow>[] = [
    {
      id: 'name',
      header: 'Tariff',
      required: true,
      width: 200,
      sortValue: (row) => row.name.toLowerCase(),
      cell: (row) => (
        <span>
          {row.name}
          {row.description !== '' && <span className={styles.rowSub}>{row.description}</span>}
        </span>
      ),
    },
    {
      id: 'mbit',
      header: 'Sells',
      width: 110,
      align: 'right',
      sortValue: (row) => row.mbit,
      cell: (row) => <span className={styles.amount}>{rateText(row.mbit)}</span>,
    },
    {
      id: 'price',
      header: 'Price',
      width: 150,
      align: 'right',
      sortValue: (row) => row.price.minor,
      cell: (row) => (
        <span className={styles.amount}>
          <Amount money={row.price} />
          <span className={styles.rowSub}>{UNIT_WORDS[row.unit]}</span>
        </span>
      ),
    },
    {
      id: 'period_price',
      header: 'One period',
      width: 160,
      align: 'right',
      sortValue: (row) => row.period_price.minor,
      cell: (row) => (
        <span className={styles.amount}>
          <Amount money={row.period_price} />
          <span className={styles.rowSub}>{PERIOD_WORDS[row.period]}</span>
        </span>
      ),
    },
    {
      id: 'subscriptions',
      header: 'Sold to',
      width: 150,
      sortValue: (row) => row.active_subscriptions,
      cell: (row) =>
        row.subscriptions === 0 ? (
          <Words>nobody yet</Words>
        ) : (
          <span>
            {plural(row.active_subscriptions, 'active plan')}
            <span className={styles.rowSub}>
              {formatCount(row.subscriptions)} agreed on these terms, ever
            </span>
          </span>
        ),
    },
  ];

  const linkColumns: readonly TableColumn<IspLink>[] = [
    {
      id: 'name',
      header: 'Link',
      required: true,
      width: 200,
      sortValue: (row) => row.name.toLowerCase(),
      cell: (row) => (
        <span>
          {row.name}
          {row.description !== '' && <span className={styles.rowSub}>{row.description}</span>}
        </span>
      ),
    },
    {
      id: 'tower',
      header: 'Tower',
      width: 140,
      sortValue: (row) => row.tower ?? '',
      cell: (row) =>
        row.tower === null ? <Words>no tower label</Words> : <span>{row.tower}</span>,
    },
    {
      id: 'where',
      header: 'Measured on',
      width: 200,
      cell: (row) =>
        row.device_id === null ? (
          <Words title="Without a device and an interface this link has no measured load — which is not a load of zero.">
            no device recorded
          </Words>
        ) : (
          <span>
            {deviceName(row.device_id)}
            <span className={styles.rowSub}>
              {row.interface ?? 'no interface recorded'}
            </span>
          </span>
        ),
    },
    {
      id: 'capacity',
      header: 'Carries',
      width: 120,
      align: 'right',
      sortValue: (row) => row.capacity_mbit,
      cell: (row) =>
        row.capacity_mbit > 0 ? (
          <span className={styles.amount}>{rateText(row.capacity_mbit)}</span>
        ) : (
          <Words>not recorded</Words>
        ),
    },
    {
      id: 'cost',
      header: 'Wholesale',
      width: 170,
      align: 'right',
      cell: (row) =>
        row.cost_per_mbit === null ? (
          <Words title="Without a wholesale cost there is no margin to work out — not a margin of zero.">
            no cost recorded
          </Words>
        ) : (
          <span className={styles.amount}>
            <Amount money={row.cost_per_mbit} />
            <span className={styles.rowSub}>
              per Mbit, {PERIOD_WORDS[row.cost_period]}
            </span>
          </span>
        ),
    },
  ];

  return (
    <div className={styles.scrollPane}>
      <Panel
        title="Tariffs"
        padding="none"
        actions={
          <Button
            size="sm"
            icon={Plus}
            onClick={() => setTariffDialog({ open: true, tariff: null })}
          >
            Add a tariff
          </Button>
        }
      >
        {tariffs.isError ? (
          <ErrorState
            size="compact"
            title="The tariffs could not be read"
            error={tariffs.error}
            onRetry={() => void tariffs.refetch()}
          />
        ) : (
          <Table
            id="isp.tariffs"
            aria-label="Tariffs"
            columns={tariffColumns}
            rows={tariffs.data ?? []}
            rowKey={(row) => row.id}
            loading={tariffs.isPending}
            maxHeight={360}
            onRowActivate={(row) => setTariffDialog({ open: true, tariff: row })}
            rowActions={(row) => [
              { label: 'Edit…', onSelect: () => setTariffDialog({ open: true, tariff: row }) },
              {
                label: 'Delete…',
                tone: 'danger',
                onSelect: () => {
                  void (async () => {
                    const ok = await confirm({
                      title: `Delete the tariff ${row.name}?`,
                      description:
                        'A tariff with an active subscription on it cannot be deleted. Subscriptions keep their own copy of what was agreed, so deleting a tariff never changes what anybody pays.',
                      confirmLabel: 'Delete',
                      tone: 'danger',
                    });
                    if (!ok) return;
                    deleteTariff.mutate(
                      { id: row.id },
                      { onSuccess: (gone) => toast.success(`${gone.name} was deleted`) },
                    );
                  })();
                },
              },
            ]}
            empty={
              <EmptyState
                icon={Package}
                size="compact"
                title="No tariffs yet"
                description="A tariff is a named plan: how many megabits, the price per megabit or for the whole plan, the currency and the period."
                action={
                  <Button
                    variant="primary"
                    icon={Plus}
                    onClick={() => setTariffDialog({ open: true, tariff: null })}
                  >
                    Add a tariff
                  </Button>
                }
              />
            }
          />
        )}
      </Panel>

      <Panel
        title="Links and tower feeds"
        padding="none"
        actions={
          <Button size="sm" icon={Plus} onClick={() => setLinkDialog({ open: true, link: null })}>
            Add a link
          </Button>
        }
      >
        {links.isError ? (
          <ErrorState
            size="compact"
            title="The links could not be read"
            error={links.error}
            onRetry={() => void links.refetch()}
          />
        ) : (
          <Table
            id="isp.links"
            aria-label="Links and tower feeds"
            columns={linkColumns}
            rows={links.data ?? []}
            rowKey={(row) => row.id}
            loading={links.isPending}
            maxHeight={360}
            onRowActivate={(row) => setLinkDialog({ open: true, link: row })}
            rowActions={(row) => [
              { label: 'Edit…', onSelect: () => setLinkDialog({ open: true, link: row }) },
              {
                label: 'Delete…',
                tone: 'danger',
                onSelect: () => {
                  void (async () => {
                    const ok = await confirm({
                      title: `Delete the link ${row.name}?`,
                      description:
                        'The customers behind it keep their tower label, and their plans stay as they are. What goes is the capacity and the wholesale cost, so this link disappears from the oversubscription and margin reports.',
                      confirmLabel: 'Delete',
                      tone: 'danger',
                    });
                    if (!ok) return;
                    deleteLink.mutate(
                      { id: row.id },
                      { onSuccess: (gone) => toast.success(`${gone.name} was deleted`) },
                    );
                  })();
                },
              },
            ]}
            empty={
              <EmptyState
                icon={Cable}
                size="compact"
                title="No links yet"
                description="A link is an upstream or a tower's feed: what it can carry, what a megabit on it costs wholesale, and the device and interface it is — which is what puts a measured figure beside the sold one."
                action={
                  <Button
                    variant="primary"
                    icon={Plus}
                    onClick={() => setLinkDialog({ open: true, link: null })}
                  >
                    Add a link
                  </Button>
                }
              />
            }
          />
        )}
      </Panel>

      <TariffDialog
        open={tariffDialog.open}
        tariff={tariffDialog.tariff}
        onClose={() => setTariffDialog({ open: false, tariff: null })}
      />
      <LinkDialog
        open={linkDialog.open}
        link={linkDialog.link}
        onClose={() => setLinkDialog({ open: false, link: null })}
      />
    </div>
  );
}
