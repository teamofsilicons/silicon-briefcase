import type { Metadata } from 'next';
import './globals.css';
import './workspace.css';
export const metadata:Metadata={title:'Briefcase · Team of Silicons',description:'Manage your organisation’s files, shared spaces, permissions, and version history with Silicon Briefcase.'};
export default function RootLayout({children}:Readonly<{children:React.ReactNode}>){return <html lang="en"><body>{children}</body></html>;}
