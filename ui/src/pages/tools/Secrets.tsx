/**
 * The three secret tools, and the one distinction they exist to make.
 *
 * `tools.secret.identify` answers two different questions — is the *format*
 * reversible, and can *this toolbox* recover the secret — and a screen that
 * collapsed them into one tick would teach the reader something false about
 * every Cisco type 6 secret in their network. So both statements are shown,
 * side by side, with their own words (`secrets.ts` writes them) and the
 * engine's own advice underneath.
 *
 * A decoded or generated value carries the engine's note wherever it appears
 * ("obfuscation with a published algorithm, not encryption"), because that
 * sentence is the whole value of the answer: knowing the password is not the
 * point, knowing that anyone with the configuration has it is.
 *
 * Nothing here is written anywhere. The inputs of these tools are not
 * remembered (`Spec.secret` in `specs.ts`), the values are not in the URL, the
 * backend writes no audit line and no history record for them, and this screen
 * adds no copy of its own.
 */

import { KeyRound, ShieldAlert, ShieldCheck } from 'lucide-react';
import type { Decoded, Encoded, GeneratedKey, Identified, Passwords } from '@/api/types/toolbox';
import { Badge } from '@/design/Badge';
import { CodeBlock } from '@/design/CodeBlock';
import { Icon, type LucideIcon } from '@/design/Icon';
import { AskedAnswer, LiveAnswer, Refusal } from './Answer';
import { useDebounced, useToolResult, type Form } from './data';
import { Fields } from './Fields';
import { CopyButton, Facts } from './Sections';
import { explainSecret, KEPT_NOWHERE, type Statement } from './secrets';
import type { Spec } from './specs';
import styles from './Tools.module.css';
import resultStyles from './Result.module.css';

/** `tools.secret.identify`: what the format is, and what Nocturne can do. */
export function Identify({ spec, form }: { spec: Spec; form: Form }) {
  const settled = useDebounced(form.values);
  const params = spec.params(settled);
  const answer = useToolResult<Identified>('tools.secret.identify', params);

  return (
    <>
      <div className={styles.formRow}>
        <Fields fields={spec.fields} values={form.values} onChange={form.set} />
      </div>
      {params === null ? (
        <p className={styles.secretIdle}>
          Paste a line from a configuration. It is read on this machine, is not stored, and does not
          reach a model.
        </p>
      ) : answer.error ? (
        <Refusal error={answer.error} />
      ) : answer.data ? (
        <Verdict found={answer.data} />
      ) : null}
    </>
  );
}

function Verdict({ found }: { found: Identified }) {
  const explained = explainSecret(found);
  return (
    <div className={styles.answer}>
      <div className={styles.verdictHead}>
        <h4 className={styles.verdictName}>{found.name}</h4>
        <Badge tone="neutral" variant="outline">
          {found.kind}
        </Badge>
      </div>

      {/* Two statements, never one. The left one is about the format, the
          right one about this toolbox; a type 6 secret says "reversible by
          design" and "Nocturne cannot recover it" at the same time, and that
          is not a contradiction. */}
      <div className={styles.statements}>
        <Statements
          title="The format"
          statement={explained.format}
          icon={found.reversible ? ShieldAlert : ShieldCheck}
        />
        <Statements title="In Nocturne" statement={explained.recovery} icon={KeyRound} />
      </div>

      {/* One row, not two: the engine's word for the strength is the first
          word of the sentence that explains it, and splitting them put the
          explanation in a column of its own where it wrapped badly. */}
      <Facts
        facts={[
          { label: 'What it is worth', value: explained.strength.detail, tone: explained.strength.tone },
          ...(found.decoder ? [{ label: 'Decoder', value: found.decoder, mono: true }] : []),
        ]}
      />
      <p className={resultStyles.note}>{explained.advice}</p>
    </div>
  );
}

function Statements({
  title,
  statement,
  icon,
}: {
  title: string;
  statement: Statement;
  icon: LucideIcon;
}) {
  return (
    <section className={styles.statement} data-tone={statement.tone}>
      <span className={styles.statementTitle}>{title}</span>
      <span className={styles.statementHead}>
        <Icon icon={icon} size={15} />
        {statement.headline}
      </span>
      <p className={styles.statementText}>{statement.detail}</p>
    </section>
  );
}

/** `tools.secret.decode`: the value, and why it was possible at all. */
export function Decode({ spec, form }: { spec: Spec; form: Form }) {
  return (
    <LiveAnswer
      id="tools.secret.decode"
      spec={spec}
      form={form}
      footer={(result) => <DecodedValue decoded={result as Decoded} />}
    />
  );
}

function DecodedValue({ decoded }: { decoded: Decoded }) {
  return (
    <div className={styles.secret}>
      <span className={styles.secretLabel}>The secret</span>
      <div className={styles.secretValue}>
        <code>{decoded.secret}</code>
        <CopyButton value={decoded.secret} label="the decoded secret" />
      </div>
      <p className={styles.secretNote}>{decoded.note}</p>
      <p className={styles.secretKept}>{KEPT_NOWHERE}</p>
    </div>
  );
}

/** `tools.secret.encode`: a value to compare with a configuration. */
export function Encode({ spec, form }: { spec: Spec; form: Form }) {
  return (
    <AskedAnswer
      id="tools.secret.encode"
      spec={spec}
      form={form}
      footer={(result) => <EncodedValue encoded={result as Encoded} />}
    />
  );
}

function EncodedValue({ encoded }: { encoded: Encoded }) {
  return (
    <div className={styles.secret}>
      <span className={styles.secretLabel}>The encoded value</span>
      <div className={styles.secretValue}>
        <code>{encoded.encoded}</code>
        <CopyButton value={encoded.encoded} label="the encoded value" />
      </div>
      <CodeBlock code={encoded.example} title="Where it goes" />
      <p className={styles.secretNote}>{encoded.note}</p>
      <p className={styles.secretKept}>{KEPT_NOWHERE}</p>
    </div>
  );
}

/** `tools.password`: passwords from the operating system's own generator. */
export function PasswordTool({ spec, form }: { spec: Spec; form: Form }) {
  return (
    <AskedAnswer
      id="tools.password"
      spec={spec}
      form={form}
      footer={(result) => <PasswordList made={result as Passwords} />}
    />
  );
}

function PasswordList({ made }: { made: Passwords }) {
  return (
    <div className={styles.secret}>
      <span className={styles.secretLabel}>
        {made.passwords.length === 1 ? 'The password' : `${made.passwords.length} passwords`}
      </span>
      <ul className={styles.passwords}>
        {made.passwords.map((password, index) => (
          <li key={index} className={styles.secretValue}>
            <code>{password}</code>
            <CopyButton value={password} label={`password ${index + 1}`} />
          </li>
        ))}
      </ul>
      <p className={styles.secretNote}>
        From the operating system’s random generator. The symbols it may use are{' '}
        <code>{made.symbols}</code>, so a device’s own rules can be checked against them.
      </p>
      <p className={styles.secretKept}>{KEPT_NOWHERE}</p>
    </div>
  );
}

/** `tools.keys.generate`: a key pair, returned once. */
export function KeyTool({ spec, form }: { spec: Spec; form: Form }) {
  return (
    <AskedAnswer
      id="tools.keys.generate"
      spec={spec}
      form={form}
      footer={(result) => <KeyPair key_={result as GeneratedKey} />}
    />
  );
}

function KeyPair({ key_ }: { key_: GeneratedKey }) {
  return (
    <div className={styles.secret}>
      <Facts
        facts={[
          { label: 'Type', value: key_.kind },
          key_.fingerprint === null
            ? {
                label: 'Fingerprint',
                value: 'not computed in this build — the terminal feature is what computes it',
                tone: 'idle' as const,
              }
            : { label: 'Fingerprint', value: key_.fingerprint, mono: true, copy: true },
        ]}
      />
      <CodeBlock code={key_.public_key} title="Public key (the authorized_keys line)" wrap />
      <CodeBlock code={key_.private_key} title="Private key (OpenSSH, unencrypted)" maxHeight={220} />
      <p className={styles.secretKept}>
        {KEPT_NOWHERE} Save the private key yourself, now: asking again makes a different pair.
      </p>
    </div>
  );
}
