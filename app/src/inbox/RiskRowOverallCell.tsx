// S10 — small presentational cell for the MissionHistory table.
// Renders the audit overall score in a single <td> with a
// colour-graded class based on standard thresholds.

interface RiskRowOverallCellProps {
  overall: number;
}

function colorClass(overall: number): string {
  if (overall >= 0.8) return "mission-history-overall--good";
  if (overall >= 0.5) return "mission-history-overall--warning";
  return "mission-history-overall--bad";
}

export default function RiskRowOverallCell({ overall }: RiskRowOverallCellProps) {
  return (
    <td className={["mission-history-overall", colorClass(overall)].join(" ")}>
      {overall.toFixed(2)}
    </td>
  );
}
