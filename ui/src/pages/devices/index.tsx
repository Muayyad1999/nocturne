/**
 * Devices — the inventory and one device's page.
 *
 * The route is `devices/:id?`: without an id this is the list, with one it is
 * that device. Both read the core's `devices.*`, `collect.*` and `profiles.*`
 * methods; `src/api/types/core.ts` documents every call they make.
 */

import { useParams } from 'react-router';
import DeviceDetail from './DeviceDetail';
import DeviceList from './DeviceList';

export default function DevicesPage() {
  const { id } = useParams();
  // Keyed on the id: moving to another device starts its page clean.
  return id ? <DeviceDetail key={id} id={id} /> : <DeviceList />;
}
