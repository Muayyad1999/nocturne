import { MessageSquare, MoreHorizontal, Pencil, Trash } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useRpcMutation } from '@/api/hooks';
import type { AiChatsRenameParams, AiDeleted, Chat } from '@/api/types/ai';
import { IconButton } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { DropdownMenu } from '@/design/DropdownMenu';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import { SearchInput } from '@/design/SearchInput';
import { Skeleton } from '@/design/Skeleton';
import { TimeAgo } from '@/design/TimeAgo';
import styles from './Assistant.module.css';

export interface ChatListProps {
  chats: readonly Chat[];
  loading: boolean;
  currentId: string | null;
  onSelect: (id: string) => void;
}

/** Past conversations, newest first: pick one, rename it, delete it. */
export function ChatList({ chats, loading, currentId, onSelect }: ChatListProps) {
  const [query, setQuery] = useState('');
  const [renaming, setRenaming] = useState<string | null>(null);
  const [title, setTitle] = useState('');

  const rename = useRpcMutation<Chat, AiChatsRenameParams>('ai.chats.rename', {
    invalidates: ['ai.chats.'],
    errorToast: 'That chat was not renamed',
  });
  const remove = useRpcMutation<AiDeleted, { id: string }>('ai.chats.delete', {
    invalidates: ['ai.chats.'],
    errorToast: 'That chat was not deleted',
  });

  const rows = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return chats;
    return chats.filter((chat) => chat.title.toLowerCase().includes(needle));
  }, [chats, query]);

  const drop = async (chat: Chat) => {
    const sure = await confirm({
      title: `Delete “${chat.title || 'this chat'}”?`,
      description:
        'The conversation and everything the assistant read for it are removed, including the secrets it learned to hide. This cannot be undone.',
      confirmLabel: 'Delete',
      tone: 'danger',
    });
    if (sure) remove.mutate({ id: chat.id });
  };

  return (
    <aside className={styles.rail} aria-label="Conversations">
      <div className={styles.railSearch}>
        <SearchInput
          size="sm"
          placeholder="Find a conversation"
          aria-label="Find a conversation"
          value={query}
          onChange={setQuery}
        />
      </div>
      {loading ? (
        <div className={styles.railLoading} aria-busy="true" aria-label="Loading conversations">
          <Skeleton height={34} shape="block" />
          <Skeleton height={34} shape="block" />
          <Skeleton height={34} shape="block" />
        </div>
      ) : rows.length === 0 ? (
        <p className={styles.railEmpty}>
          {chats.length === 0
            ? 'Nothing has been asked yet. New chat, at the top right, starts one.'
            : 'No conversation matches that.'}
        </p>
      ) : (
        <ul className={styles.chats}>
          {rows.map((chat) => (
            <li key={chat.id}>
              {renaming === chat.id ? (
                <form
                  className={styles.renaming}
                  onSubmit={(event) => {
                    event.preventDefault();
                    rename.mutate({ id: chat.id, title });
                    setRenaming(null);
                  }}
                >
                  <Input
                    size="sm"
                    autoFocus
                    aria-label="Chat name"
                    value={title}
                    onChange={(event) => setTitle(event.target.value)}
                    onBlur={() => setRenaming(null)}
                  />
                </form>
              ) : (
                <div className={styles.chatRow} data-current={chat.id === currentId || undefined}>
                  <button
                    type="button"
                    className={styles.chatButton}
                    aria-current={chat.id === currentId ? 'true' : undefined}
                    onClick={() => onSelect(chat.id)}
                  >
                    <Icon icon={MessageSquare} size={13} className={styles.chatIcon} />
                    <span className={styles.chatTitle}>{chat.title || 'New chat'}</span>
                    <TimeAgo className={styles.chatWhen} at={chat.updated_at} />
                  </button>
                  <DropdownMenu
                    aria-label={`Actions for ${chat.title || 'this chat'}`}
                    placement="bottom-end"
                    trigger={
                      <IconButton
                        icon={MoreHorizontal}
                        size="sm"
                        aria-label={`Actions for ${chat.title || 'this chat'}`}
                        tooltip={false}
                      />
                    }
                    items={[
                      {
                        label: 'Rename',
                        icon: Pencil,
                        onSelect: () => {
                          setTitle(chat.title);
                          setRenaming(chat.id);
                        },
                      },
                      {
                        label: 'Delete',
                        icon: Trash,
                        tone: 'danger',
                        onSelect: () => void drop(chat),
                      },
                    ]}
                  />
                </div>
              )}
            </li>
          ))}
        </ul>
      )}
    </aside>
  );
}
