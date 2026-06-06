import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Download } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import {
  migrationDetect,
  migrationImportCcswitch,
  migrationImportTakoCli,
  type MigrationDetect,
} from "@/lib/api/tako";
import { toast } from "sonner";

export function MigrationPromptDialog() {
  const { t } = useTranslation();
  const [detect, setDetect] = useState<MigrationDetect | null>(null);
  const [isOpen, setIsOpen] = useState(false);
  const [importing, setImporting] = useState(false);

  useEffect(() => {
    migrationDetect()
      .then((d) => {
        if (d.ccswitch_available || d.tako_cli_available) {
          setDetect(d);
          setIsOpen(true);
        }
      })
      .catch(console.error);
  }, []);

  const handleImport = async () => {
    if (!detect) return;
    setImporting(true);
    try {
      if (detect.ccswitch_available) {
        await migrationImportCcswitch();
        toast.success(t("migration.ccImported"));
      }
      if (detect.tako_cli_available) {
        await migrationImportTakoCli();
        toast.success(t("migration.takoImported"));
      }
      setIsOpen(false);
    } catch (e) {
      toast.error(`${t("migration.importFailed")}: ${e}`);
    } finally {
      setImporting(false);
    }
  };

  if (!detect) return null;

  return (
    <Dialog open={isOpen} onOpenChange={setIsOpen}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Download className="h-5 w-5" />
            {t("migration.title")}
          </DialogTitle>
          <DialogDescription className="space-y-2 text-left">
            {detect.ccswitch_available && <p>{t("migration.ccswitch")}</p>}
            {detect.tako_cli_available && (
              <p>
                {t("migration.takoAccount")}
                {detect.tako_account_id ? ` (${detect.tako_account_id})` : ""}
              </p>
            )}
          </DialogDescription>
        </DialogHeader>
        <DialogFooter className="flex gap-2 sm:justify-end">
          <Button
            variant="outline"
            onClick={() => setIsOpen(false)}
            disabled={importing}
          >
            {t("migration.skip")}
          </Button>
          <Button onClick={handleImport} disabled={importing}>
            {importing ? t("migration.importing") : t("migration.import")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
