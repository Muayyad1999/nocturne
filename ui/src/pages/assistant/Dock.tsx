import { Sparkles } from 'lucide-react';
import { DockComposer, DockTranscript } from '@/app/AssistantDock';
import { EmptyState } from '@/design/EmptyState';

/**
 * What the assistant dock shows inside its frame. Stub: the Assistant slice
 * replaces this with the live conversation, keeping DockTranscript and
 * DockComposer as the two regions.
 */
export default function AssistantDockContent() {
  return (
    <>
      <DockTranscript>
        <EmptyState
          icon={Sparkles}
          title="The assistant is not connected yet"
          description="It will answer questions about your network and propose changes for you to review. Nothing will run without your approval."
        />
      </DockTranscript>
      <DockComposer disabled placeholder="Not available in this build" />
    </>
  );
}
