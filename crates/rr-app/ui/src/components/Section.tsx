import type { ReactNode } from "react";

export function Section({ title, children, open = true, badge }: { title: string; children: ReactNode; open?: boolean; badge?: ReactNode }) {
  return (
    <details className="section" open={open}>
      <summary>
        <span>{title}</span>
        {badge}
      </summary>
      <div className="section-body">{children}</div>
    </details>
  );
}