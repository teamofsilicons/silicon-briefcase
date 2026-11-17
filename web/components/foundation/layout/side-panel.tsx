"use client";

/**
 * The side column an action opens (the content layout rule, redesign/decisions.md). Pass it as a Page's `panel`, with
 * the same `open` as the Page's `panelOpen`:
 *
 *   <Page header={…} panel={<SidePanel open={open} onOpenChange={setOpen} title="…">…</SidePanel>} panelOpen={open}>
 *
 * From 1024px it sits beside the page's column (the Page animates the page open; this panel slides and fades in), with
 * its title and Close at the top. Opening moves focus to Close; Close or Escape shuts it and focus goes back to
 * `returnFocus()` or, failing that, to whatever had focus when it opened. Under 1024px the same content opens in
 * Silicon UI's bottom Drawer instead (no room beside the page), and focus is the Drawer's.
 *
 * Keep the content rendered while it closes (Silicon UI's `useDrawerItem` keeps the last item), so it fades out
 * rather than vanishing.
 */
import { useEffect, useEffectEvent, useId, useRef, useSyncExternalStore, type ReactNode } from "react";
import { X } from "lucide-react";
import { Button } from "@teamofsilicons/silicon-ui/button";
import { Drawer, DrawerContent } from "@teamofsilicons/silicon-ui/drawer";
import styles from "./side-panel.module.css";

/** Under 1024px the panel opens as a drawer (layout.module.css hides the side column at the same width). */
const NARROW = "(max-width: 1023px)";
const subscribeNarrow = (change: () => void) => {
  const media = window.matchMedia(NARROW);
  media.addEventListener("change", change);
  return () => media.removeEventListener("change", change);
};
const readNarrow = () => window.matchMedia(NARROW).matches;

export interface SidePanelProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: string;
  /** Where focus goes when the panel closes, such as the tile that opened it (it may have moved or re-rendered). */
  returnFocus?: () => HTMLElement | null | undefined;
  /** On the drawer under 1024px (placement only). */
  drawerClassName?: string;
  children: ReactNode;
}

export function SidePanel({ open, onOpenChange, title, description, returnFocus, drawerClassName, children }: SidePanelProps) {
  const narrow = useSyncExternalStore(subscribeNarrow, readNarrow, () => false);
  const beside = open && !narrow;
  const closeRef = useRef<HTMLButtonElement>(null);
  const opener = useRef<HTMLElement | null>(null);
  const titleId = useId();

  const giveFocusBack = () => {
    const target = returnFocus?.() ?? opener.current;
    if (target?.isConnected) target.focus({ preventScroll: true });
  };
  const onEscape = useEffectEvent(() => {
    onOpenChange(false);
    giveFocusBack();
  });

  // Beside the page: focus moves in on open; Escape closes (unless something inside handled it first).
  useEffect(() => {
    if (!beside) return;
    opener.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    closeRef.current?.focus({ preventScroll: true });
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      onEscape();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [beside]);

  if (narrow) {
    return (
      <Drawer open={open} onOpenChange={onOpenChange}>
        <DrawerContent
          side="bottom"
          title={title}
          description={description}
          className={drawerClassName}
          onCloseAutoFocus={event => {
            const target = returnFocus?.();
            if (!target?.isConnected) return;
            event.preventDefault();
            target.focus({ preventScroll: true });
          }}
        >
          {children}
        </DrawerContent>
      </Drawer>
    );
  }

  return (
    <section className={styles.panel} data-side-panel="" data-open={open ? "" : undefined} aria-labelledby={titleId}>
      <header className={styles.head}>
        <div className={styles.headText}>
          <h2 id={titleId} className={styles.title}>{title}</h2>
          {description ? <p className={styles.description}>{description}</p> : null}
        </div>
        <Button ref={closeRef} variant="ghost" size="sm" aria-label="Close" onClick={() => { onOpenChange(false); giveFocusBack(); }}>
          <X size={16} strokeWidth={2} aria-hidden="true" />
        </Button>
      </header>
      <div className={styles.body}>{children}</div>
    </section>
  );
}
