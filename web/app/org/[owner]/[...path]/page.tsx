import { locationPage } from "@/lib/server/location";
export default async function LegacyLocationPage({params}:{params:Promise<{owner:string;path:string[]}>}) {const {owner,path}=await params;return locationPage(owner,path,true);}
