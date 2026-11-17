/**
 * Silicon UI's ButtonLink with the app's router: navigation goes through a link, never a button with a router push.
 * Same variants and sizes as <Button>; the hosted pages' button styles (data-variant) apply.
 *
 *   <ButtonLink href={paths.signIn} size="lg">Sign In</ButtonLink>
 *   <ButtonLink href="https://apps.teamofsilicons.com" variant="secondary" external>Make an App</ButtonLink>
 */
import Link from "next/link";
import { ButtonLink as SiliconButtonLink, type ButtonLinkProps as SiliconButtonLinkProps } from "@teamofsilicons/silicon-ui/button-link";

export interface ButtonLinkProps extends Omit<SiliconButtonLinkProps, "as"> {
  /** A plain <a> for other sites (no client-side navigation, no prefetch). */
  external?: boolean;
}

export function ButtonLink({ external, ...props }: ButtonLinkProps) {
  return <SiliconButtonLink {...props} as={external ? undefined : Link} />;
}
