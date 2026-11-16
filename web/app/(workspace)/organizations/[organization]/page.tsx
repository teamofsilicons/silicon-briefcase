import { OrganizationWorkspace } from "@/components/briefcase/organizations";
export default async function OrganizationPage({ params }: { params: Promise<{ organization: string }> }) {
  const { organization } = await params;
  return <OrganizationWorkspace key={organization} organization={organization} />;
}
