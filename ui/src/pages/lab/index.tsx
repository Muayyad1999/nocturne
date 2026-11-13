/**
 * Lab — networks simulated inside Nocturne.
 *
 * The route is `lab/:id?`: without an id this is the list of labs, with one
 * it is that lab's canvas, consoles and reachability. Both read the `labs.*`
 * methods documented in `src/api/types/lab.ts`; the consoles are ordinary
 * `terminal.*` sessions, because a lab node is an inventory device.
 *
 * Nothing on this screen is ever deployed: the simulator runs in the
 * backend's own process and a lab device has no address in the real world.
 */

import { useParams } from 'react-router';
import LabEditor from './Editor';
import LabList from './LabList';

export default function LabPage() {
  const { id } = useParams();
  // Keyed on the id: moving to another lab starts its screen clean.
  return id ? <LabEditor key={id} id={id} /> : <LabList />;
}
