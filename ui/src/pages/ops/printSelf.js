/*
 * The script of the print frame (PrintFrame.tsx), and the only script that
 * frame may run.
 *
 * A page cannot ask a frame of another origin to print, and a document made
 * from device data is never given the application's origin. So the frame
 * prints itself: this script takes the document from the page that created
 * the frame, puts it where the empty one was, and calls print().
 *
 * What keeps that document inert in a frame where a script runs:
 *   - it is parsed by DOMParser, where scripting is off; the script elements
 *     of such a document are already spent when they are moved in here;
 *   - the frame's content security policy (PrintFrame.tsx) allows this file
 *     and nothing else: no other script, no inline handler, no javascript:
 *     address, no frame, no request of any kind;
 *   - pragmas are removed below, for the one thing a policy cannot stop.
 *
 * Plain JavaScript in a file of its own because the application's policy
 * only runs scripts served from its own address: not inline, not a blob. It
 * is copied into the build as it is, so it keeps to syntax every supported
 * WebView runs.
 */
(() => {
  'use strict';

  const host = window.parent;
  const tell = (state) => host.postMessage({ nocturnePrint: state }, '*');

  const onDocument = (event) => {
    const message = event.data;
    if (event.source !== host || !message || message.nocturnePrint !== 'document') return;
    // One document per frame: after this one is in, nothing more is taken.
    removeEventListener('message', onDocument);
    try {
      const parsed = new DOMParser().parseFromString(String(message.html), 'text/html');
      // `<meta http-equiv="refresh">` would send this frame to another page,
      // which would then be the one running script here. Navigation is the
      // one thing a content security policy does not govern, and no pragma is
      // of any use to a page on its way to the printer.
      for (const pragma of parsed.querySelectorAll('meta[http-equiv]')) pragma.remove();
      document.replaceChild(document.adoptNode(parsed.documentElement), document.documentElement);
    } catch (error) {
      tell('failed');
      return;
    }
    // In a task of its own, so the document is in place and laid out before
    // print() takes over this frame for as long as the dialog is open.
    setTimeout(() => {
      try {
        window.print();
      } catch (error) {
        tell('failed');
      }
    }, 0);
  };

  addEventListener('message', onDocument);
  tell('ready');
})();
