import { ContactsPage } from "@/components/briefcase/contacts";
import { requireSession } from "@/lib/server/rsc";
export default async function Page() { await requireSession(); return <ContactsPage />; }
