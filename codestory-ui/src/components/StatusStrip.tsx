type StatusStripProps = {
  status: string;
  indexProgress: { current: number; total: number } | null;
};

export function StatusStrip({ status, indexProgress }: StatusStripProps) {
  return (
    <div className="status-strip">
      <span>{status}</span>
      {indexProgress && (
        <span>
          Indexing {indexProgress.current}/{indexProgress.total}
        </span>
      )}
    </div>
  );
}
