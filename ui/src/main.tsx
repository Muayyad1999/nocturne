import '@fontsource-variable/inter';
import '@fontsource-variable/jetbrains-mono';
import './design/tokens.css';
import './design/base.css';
import './design/control.css';

import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { connect } from './api/client';
import { bindQueryClient, createQueryClient } from './api/queryClient';
import { captureToken } from './api/token';
import { App } from './app/App';
import { createAppRouter } from './app/routes';
import { bindTheme } from './design/theme';

bindTheme();

// Server mode: take the access token out of the address before the hash
// router reads the fragment as a route.
captureToken();

const queryClient = createQueryClient();
bindQueryClient(queryClient);
const router = createAppRouter();

// Open the connection now, so the status bar is truthful from the first frame.
// A failure shows there as "Engine offline" and again on the first call.
connect().catch(() => undefined);

const root = document.getElementById('root');
if (!root) throw new Error('index.html has no #root element');

createRoot(root).render(
  <StrictMode>
    <App router={router} queryClient={queryClient} />
  </StrictMode>,
);
