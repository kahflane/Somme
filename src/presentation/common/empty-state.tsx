import type { ReactNode } from "react";
import classes from "@/presentation/common/page.module.css";

interface Props {
  icon: ReactNode;
  title: string;
  description?: string;
  action?: ReactNode;
  narrow?: boolean;
}

export function EmptyBlock({ icon, title, description, action, narrow }: Props) {
  return (
    <div className={classes.empty}>
      <div className={classes.emptyTile}>{icon}</div>
      <div className={classes.emptyTitle}>{title}</div>
      {description && (
        <div className={`${classes.emptyText} ${narrow ? classes.emptyTextNarrow : ""}`}>
          {description}
        </div>
      )}
      {action && <div className={classes.emptyAction}>{action}</div>}
    </div>
  );
}
