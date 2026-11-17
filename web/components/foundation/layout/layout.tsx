/**
 * Layout primitives for the workspace's pages, copied from Silicon Accounts (web/components/foundation/layout):
 *   <Page>            the one page container: a narrow centred column, a column + aside, or a column whose side
 *                     column an action opens (with SidePanel, ./side-panel.tsx); gutters and the room at the end
 *   <PageHeader>      the page's h1 in the display face, a line of context, actions, a back link
 *   <Section>         a titled region (h2) with an optional description and actions
 *   <Stack>/<Cluster>/<Grid>   spacing on the 4px grid
 *   <Surface>         a bordered squircle region (never nest them)
 *   <SettingsGroup>/<SettingsRow>   one bordered group of divided rows (label + description, control on the right)
 *   <DescriptionList>/<DescriptionItem>   label and value pairs
 *   <SkipLink>        "Skip to content", the first Tab stop of a frame (the account shell, the docs)
 *
 * Server-safe (no hooks except useId): usable from server and client components.
 */
import Link from "next/link";
import { useId, type CSSProperties, type HTMLAttributes, type ReactNode } from "react";
import { ArrowLeft } from "lucide-react";
import styles from "./layout.module.css";

const cx = (...names: Array<string | false | null | undefined>) => names.filter(Boolean).join(" ");

type Gap = 1 | 2 | 3 | 4 | 5 | 6 | 8 | 10 | 12;
const gap = (value: Gap | undefined, fallback: Gap) => `var(--space-${value ?? fallback})`;

export interface PageProps {
  /**
   * How wide the page is, by how much it holds (redesign/decisions.md, the content layout rule):
   *   "narrow"   one centred column (--page-narrow), for most pages; "reading" is the same column
   *   "default"  the full page width (--page-max), for grids that need it
   * A page with `aside` or `panel` sets its own width.
   */
  width?: "narrow" | "reading" | "default";
  /** Above the columns: the PageHeader (or Home's greeting). With `aside` or `panel` it spans both columns. */
  header?: ReactNode;
  /** Content that supports the page from the start: the page is wide, `children` beside this side column. */
  aside?: ReactNode;
  /** Names the side column for screen readers, such as "About your account". */
  asideLabel?: string;
  /**
   * Content an action opens (a SidePanel from ./side-panel): the page is the narrow column until `panelOpen`, then it
   * widens and the column glides to the start as the side column comes in (layout.module.css .page[data-layout=panel]).
   * Under 1024px the side column stays shut and the SidePanel opens a bottom drawer instead.
   */
  panel?: ReactNode;
  panelOpen?: boolean;
  className?: string;
  children: ReactNode;
  /** Accessible label when the page has no PageHeader. */
  label?: string;
}

/** The page container. One per page; the shell provides the landmark (<main>) and --page-end is the room below. */
export function Page({ width = "narrow", header, aside, asideLabel, panel, panelOpen = false, className, children, label }: PageProps) {
  const layout = panel !== undefined ? "panel" : aside !== undefined ? "aside" : "column";
  return (
    <div className={cx(styles.page, className)} data-width={width === "reading" ? "narrow" : width} data-layout={layout} data-panel-open={layout === "panel" && panelOpen ? "" : undefined} aria-label={label}>
      {header}
      {layout === "column" ? children : (
        <div className={styles.columns}>
          <div className={styles.mainColumn}>{children}</div>
          {layout === "aside" ? (
            <aside className={styles.sideColumn} aria-label={asideLabel}>{aside}</aside>
          ) : (
            <div className={styles.panelColumn} inert={!panelOpen}>{panel}</div>
          )}
        </div>
      )}
    </div>
  );
}

export interface PageHeaderProps {
  title: ReactNode;
  /** One line of context. Skip it when the content explains itself. */
  description?: ReactNode;
  /** Actions at the end (one primary at most). */
  actions?: ReactNode;
  /** A back link above the title (for nested pages such as an app's detail). */
  back?: { href: string; label: string };
  /** "entity" for a title someone typed (a file's name), which can be long: it wraps anywhere. */
  size?: "display" | "entity";
  className?: string;
  children?: ReactNode;
}

export function PageHeader({ title, description, actions, back, size = "display", className, children }: PageHeaderProps) {
  return (
    <header className={cx(styles.header, className)} data-size={size}>
      <div className={styles.headerText}>
        {back ? (
          <Link href={back.href} className={styles.back}>
            <ArrowLeft size={16} strokeWidth={2} aria-hidden="true" />
            {back.label}
          </Link>
        ) : null}
        <h1 className={styles.title}>{title}</h1>
        {description ? <p className={styles.description}>{description}</p> : null}
        {children}
      </div>
      {actions ? <div className={styles.actions}>{actions}</div> : null}
    </header>
  );
}

export interface SectionProps extends Omit<HTMLAttributes<HTMLElement>, "title"> {
  title: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
  /** Hide the heading visually (it still names the region). */
  hideTitle?: boolean;
  /** Heading level, h2 by default. */
  level?: 2 | 3;
}

export function Section({ title, description, actions, hideTitle, level = 2, className, children, ...rest }: SectionProps) {
  const id = `section-${useId().replace(/:/g, "")}`;
  const Heading = level === 3 ? "h3" : "h2";
  return (
    <section {...rest} className={cx(styles.section, className)} aria-labelledby={id}>
      <div className={cx(styles.sectionHeader, hideTitle && "sr-only")}>
        <div className={styles.sectionText}>
          <Heading id={id} className={styles.sectionTitle}>{title}</Heading>
          {description ? <p className={styles.sectionDescription}>{description}</p> : null}
        </div>
        {actions ? <div className={styles.actions}>{actions}</div> : null}
      </div>
      {children}
    </section>
  );
}

export function Stack({ gap: size, className, children }: { gap?: Gap; className?: string; children: ReactNode }) {
  return <div className={cx(styles.stack, className)} style={{ gap: gap(size, 4) }}>{children}</div>;
}

export function Cluster({ gap: size, className, children, justify = "start" }: { gap?: Gap; className?: string; children: ReactNode; justify?: "start" | "end" | "between" | "center" }) {
  const justifyContent = { start: "flex-start", end: "flex-end", between: "space-between", center: "center" }[justify];
  return <div className={cx(styles.cluster, className)} style={{ gap: gap(size, 3), justifyContent }}>{children}</div>;
}

/** A responsive grid: as many columns as fit at `min` px each. */
export function Grid({ gap: size, min = 260, className, children }: { gap?: Gap; min?: number; className?: string; children: ReactNode }) {
  return <div className={cx(styles.grid, className)} style={{ gap: gap(size, 4), "--grid-min": `${min}px` } as CSSProperties}>{children}</div>;
}

export interface SurfaceProps extends HTMLAttributes<HTMLDivElement> {
  padding?: "none" | "sm" | "md";
}

/** A bordered squircle region. Cards rest on a border, never a shadow (Arc). */
export function Surface({ padding = "md", className, children, ...rest }: SurfaceProps) {
  return <div {...rest} data-sq="surface" className={cx(styles.surface, className)} data-padding={padding}>{children}</div>;
}

export function SettingsGroup({ className, children, label }: { className?: string; children: ReactNode; label?: string }) {
  return <div data-sq="surface" className={cx(styles.group, className)} role={label ? "group" : undefined} aria-label={label}>{children}</div>;
}

export interface SettingsRowProps {
  label: ReactNode;
  description?: ReactNode;
  /** Keep a small control (a switch) beside the text on phones too, instead of under it. */
  inline?: boolean;
  /** The control (switch, button, select). The render function receives ids to wire aria-labelledby/-describedby. */
  children?: ReactNode | ((ids: { labelId: string; descriptionId: string }) => ReactNode);
  className?: string;
}

/** One row of a settings group. */
export function SettingsRow({ label, description, inline, children, className }: SettingsRowProps) {
  const uid = useId().replace(/:/g, "");
  const ids = { labelId: `row-${uid}-label`, descriptionId: `row-${uid}-description` };
  const control = typeof children === "function" ? children(ids) : children;
  return (
    <div className={cx(styles.row, className)} data-inline={inline ? "" : undefined}>
      <div className={styles.rowText}>
        <span id={ids.labelId} className={styles.rowLabel}>{label}</span>
        {description ? <span id={ids.descriptionId} className={styles.rowDescription}>{description}</span> : null}
      </div>
      {control ? <div className={styles.rowControl}>{control}</div> : null}
    </div>
  );
}

export function DescriptionList({ className, children }: { className?: string; children: ReactNode }) {
  return <dl className={cx(styles.list, className)}>{children}</dl>;
}

export function DescriptionItem({ label, children }: { label: ReactNode; children: ReactNode }) {
  return (
    <div className={styles.item}>
      <dt>{label}</dt>
      <dd>{children}</dd>
    </div>
  );
}

/**
 * "Skip to content": a frame's first Tab stop, out of sight until keyboard focus reaches it (styles/base.css
 * `.skip-link`). A squircle like every rounded surface, so it carries `data-sq`; its colours are --sq-fill/--sq-stroke.
 */
export function SkipLink({ href, children = "Skip to content" }: { href: `#${string}`; children?: ReactNode }) {
  return <a className="skip-link" data-sq="surface" href={href}>{children}</a>;
}
