/**
 * Changes — the one path by which Nocturne configures a device.
 *
 * The route is `changes/:id?`: without an id this is the list, with one it is
 * that change, from the commands nobody has read yet to the record of what
 * happened on every device. Both read `changes.*`; the backend contract is
 * `docs/dev/features/changes.md` and `src/api/types/changes.ts`.
 */

import { useParams } from 'react-router';
import ChangeList from './ChangeList';
import ChangePage from './ChangePage';

export default function ChangesPage() {
  const { id } = useParams();
  // Keyed on the id: moving to another change starts its page clean, so a
  // typed confirmation or a read plan never carries over to another change.
  return id ? <ChangePage key={id} id={id} /> : <ChangeList />;
}
