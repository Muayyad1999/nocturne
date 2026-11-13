import type { ComponentProps, CSSProperties } from 'react';
import { cx } from '@/lib/cx';
import { useFieldControl } from '../Field';
import styles from './Slider.module.css';

export interface SliderProps
  extends Omit<ComponentProps<'input'>, 'type' | 'value' | 'onChange' | 'min' | 'max' | 'step'> {
  value: number;
  onChange: (value: number) => void;
  min?: number;
  max?: number;
  step?: number;
  /**
   * Shows the current value beside the track, formatted by this function
   * (use `lib/format`). It is also what screen readers announce.
   */
  formatValue?: (value: number) => string;
}

/** A native range input: a value picked along a track. `ref` reaches the `<input>`. */
export function Slider({
  value,
  onChange,
  min = 0,
  max = 100,
  step = 1,
  formatValue,
  className,
  ...rest
}: SliderProps) {
  const props = useFieldControl(rest);
  const fill = max > min ? ((value - min) / (max - min)) * 100 : 0;
  const text = formatValue?.(value);

  return (
    <div className={cx(styles.slider, className)} data-disabled={props.disabled || undefined}>
      <input
        {...props}
        type="range"
        className={styles.input}
        style={{ '--fill': `${Math.min(100, Math.max(0, fill))}%` } as CSSProperties}
        min={min}
        max={max}
        step={step}
        value={value}
        aria-valuetext={text}
        onChange={(event) => onChange(event.target.valueAsNumber)}
      />
      {text !== undefined && <output className={styles.value}>{text}</output>}
    </div>
  );
}
