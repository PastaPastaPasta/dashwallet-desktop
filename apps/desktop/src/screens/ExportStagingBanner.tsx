import { DashButton } from "../components/basics";
import { dismissExportStaging, useStuckExportStaging } from "../app/exportStaging";
import { useT } from "../i18n";

/** The unencrypted temporary file of an unfinished Dash Core export that the start could not
 * delete: its full path, to select and delete by hand, until dismissed. */
export function ExportStagingBanner() {
  const stuck = useStuckExportStaging();
  const tr = useT();
  return (
    <>
      {stuck.map((path) => (
        <section
          key={path}
          className="protocol-advisory-banner card-l1"
          data-testid="export-staging-banner"
          aria-label={tr("home.exportStaging.label")}
        >
          <svg className="protocol-advisory-icon" viewBox="0 0 20 20" aria-hidden="true">
            <circle cx="10" cy="10" r="7.5" />
            <path d="M10 6v5M10 13.5v.5" />
          </svg>
          <div className="protocol-advisory-body" role="status">
            <p className="protocol-advisory-text" data-testid="export-staging-text">
              {tr.rich("home.exportStaging.notDeleted", {
                path: (
                  <span className="mono export-staging-path" data-testid="export-staging-path">
                    {path}
                  </span>
                ),
              })}
            </p>
          </div>
          <div className="protocol-advisory-actions">
            <DashButton variant="tintedGray" size="small" data-testid="export-staging-dismiss" onClick={() => dismissExportStaging(path)}>
              {tr("home.exportStaging.dismiss")}
            </DashButton>
          </div>
        </section>
      ))}
    </>
  );
}
