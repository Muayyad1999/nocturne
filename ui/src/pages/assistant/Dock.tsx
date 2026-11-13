/**
 * What the assistant dock shows inside its frame: the current conversation,
 * live, beside whatever screen the user is on.
 *
 * It is the same `Conversation` the full screen renders, so the transcript,
 * the tool steps and the approvals behave identically in both. The dock has no
 * room for the models form, so its setup card sends the user to `/assistant`
 * with the door they chose already open.
 */

import { useNavigate } from 'react-router';
import type { LocalEndpoint, ProviderPreset } from '@/api/types/ai';
import { Conversation } from './Conversation';
import { useChatBootstrap } from './useChats';

export default function AssistantDockContent() {
  const navigate = useNavigate();
  const bootstrap = useChatBootstrap();

  const addProvider = (preset: ProviderPreset | null, local?: LocalEndpoint) => {
    const query = new URLSearchParams({ tab: 'models' });
    if (preset) query.set('add', preset.id);
    if (local) query.set('at', local.base_url);
    void navigate(`/assistant?${query.toString()}`);
  };

  return <Conversation bootstrap={bootstrap} onAddProvider={addProvider} variant="dock" />;
}
