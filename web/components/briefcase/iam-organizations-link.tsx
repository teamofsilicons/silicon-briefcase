import { ArrowUpRight } from 'lucide-react';

export default function IamOrganizationsLink() {
  return (
    <div className="space-y-2 text-sm">
      <a
        className="inline-flex items-center gap-1 text-primary underline-offset-4 hover:underline focus-visible:outline-2 focus-visible:outline-offset-4"
        href="https://iam.teamofsilicons.com/"
        target="_blank"
        rel="noopener noreferrer"
      >
        Create or manage organisations in IAM
        <ArrowUpRight size={16} aria-hidden="true" />
        <span className="sr-only"> (opens in a new tab)</span>
      </a>
      <p className="text-muted-foreground">
        Manage organisations and invitations in IAM. After joining or creating
        one, review your IAM access selection to share it with Briefcase.
      </p>
    </div>
  );
}
