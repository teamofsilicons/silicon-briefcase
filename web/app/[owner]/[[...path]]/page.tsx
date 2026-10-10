import { locationPage } from "@/lib/server/location";
export default async function LocationPage({params}:{params:Promise<{owner:string;path?:string[]}>}) {const {owner,path=[]}=await params;return locationPage(owner,path);}
