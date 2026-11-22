/**
 * uPlot, in an environment it can load in.
 *
 * While its module is evaluated, uPlot runs
 * `new Intl.NumberFormat(navigator.language)`. WebKitGTK reports the process
 * locale there, and on a Linux desktop whose locale is unset or C that is
 * `"C"`: a RangeError, thrown before a single chart exists, which took the
 * whole Monitoring screen with it.
 *
 * There is no option to pass, so the only cure is for `navigator` to be right
 * before uPlot is evaluated. A module's dependencies are evaluated in the
 * order they are written, which is why the repair is a module of its own and
 * why it is the first line here. Import uPlot from this file, never from the
 * package, and keep the order.
 */

import './navigatorLocale';
import uPlot from 'uplot';
import 'uplot/dist/uPlot.min.css';

export default uPlot;
