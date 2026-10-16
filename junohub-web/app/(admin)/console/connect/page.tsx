import { redirect } from "next/navigation";

export default function IntegrationIndexPage() {
  redirect("/console/connect/providers");
}
