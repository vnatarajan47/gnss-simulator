"use client";

import {
  MAX_INTERVAL_HOURS,
  formatStep,
  formatTimestamp,
  unixSecondsToInput,
  type IntervalProblem,
} from "@/lib/interval";

/**
 * Window selection and the shared time cursor.
 *
 * The slider is deliberately an index into the series' epoch array rather than
 * a timestamp: every view is handed the same integer, so the sky-plot markers
 * and the DOP cursor are synchronised by construction and not by two
 * independent time-to-position conversions that could round differently.
 */
export default function TimeControls({
  startValue,
  endValue,
  minValue,
  maxValue,
  onStartChange,
  onEndChange,
  problem,
  epochs,
  stepS,
  cursor,
  onCursorChange,
  disabled,
}: {
  /** `datetime-local` values, treated as UTC. */
  startValue: string;
  endValue: string;
  minValue: string;
  maxValue: string;
  onStartChange: (value: string) => void;
  onEndChange: (value: string) => void;
  problem: IntervalProblem | null;
  /** Sampled epochs as Unix seconds; empty until a series has been computed. */
  epochs: number[];
  stepS: number;
  cursor: number;
  onCursorChange: (index: number) => void;
  disabled: boolean;
}) {
  const hasSeries = epochs.length > 0;
  const currentS = hasSeries ? epochs[Math.min(cursor, epochs.length - 1)] : null;

  return (
    <div style={styles.wrapper}>
      <div style={styles.row}>
        <label style={styles.label}>
          Start (UTC)
          <input
            type="datetime-local"
            value={startValue}
            min={minValue}
            max={maxValue}
            onChange={(event) => onStartChange(event.target.value)}
            style={styles.input}
          />
        </label>
        <label style={styles.label}>
          End (UTC)
          <input
            type="datetime-local"
            value={endValue}
            min={minValue}
            max={maxValue}
            onChange={(event) => onEndChange(event.target.value)}
            style={styles.input}
          />
        </label>
      </div>

      {problem ? (
        <p style={styles.error}>{problem.message}</p>
      ) : (
        <p style={styles.hint}>
          Up to {MAX_INTERVAL_HOURS} h.
          {hasSeries && (
            <>
              {" "}
              Sampling {epochs.length} epochs every {formatStep(stepS)}.
            </>
          )}
        </p>
      )}

      <label style={styles.sliderLabel}>
        <span style={styles.sliderHeader}>
          <span>Time</span>
          <strong style={styles.clock}>
            {currentS === null ? "—" : `${formatTimestamp(currentS)} UTC`}
          </strong>
        </span>
        <input
          type="range"
          min={0}
          max={Math.max(epochs.length - 1, 0)}
          step={1}
          value={Math.min(cursor, Math.max(epochs.length - 1, 0))}
          onChange={(event) => onCursorChange(Number(event.target.value))}
          disabled={disabled || !hasSeries}
          style={styles.slider}
          aria-label="Time within the selected window"
          aria-valuetext={currentS === null ? undefined : `${formatTimestamp(currentS)} UTC`}
        />
        {hasSeries && (
          <span style={styles.ends}>
            <span>{unixSecondsToInput(epochs[0]).slice(11)}</span>
            <span>{unixSecondsToInput(epochs[epochs.length - 1]).slice(11)}</span>
          </span>
        )}
      </label>
    </div>
  );
}

const styles: Record<string, React.CSSProperties> = {
  wrapper: { display: "flex", flexDirection: "column", gap: 6, marginBottom: 12 },
  row: { display: "flex", gap: 10, flexWrap: "wrap" },
  label: { display: "flex", flexDirection: "column", gap: 4, fontSize: 13, flex: "1 1 180px" },
  input: { padding: 4, fontSize: 13, width: "100%", boxSizing: "border-box" },
  hint: { fontSize: 11, color: "#6b7280", margin: 0 },
  error: { fontSize: 12, color: "#b91c1c", margin: 0 },
  sliderLabel: { display: "flex", flexDirection: "column", gap: 2, fontSize: 13, marginTop: 2 },
  sliderHeader: { display: "flex", justifyContent: "space-between", alignItems: "baseline" },
  clock: { fontVariantNumeric: "tabular-nums", fontSize: 12, color: "#111827" },
  slider: { width: "100%" },
  ends: {
    display: "flex",
    justifyContent: "space-between",
    fontSize: 10,
    color: "#9ca3af",
    fontVariantNumeric: "tabular-nums",
  },
};
