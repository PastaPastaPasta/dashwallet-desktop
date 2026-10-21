import type { Page } from "@playwright/test";

// Edit Profile's test pictures and the file dialog's answer (DP4-03), shared by
// edit-profile.spec.ts and visual.spec.ts.

/** A 60 × 40 PNG, blue on the left half and orange on the right: the crop's file. */
export const WIDE_PNG =
  "iVBORw0KGgoAAAANSUhEUgAAADwAAAAoCAIAAAAt2Q6oAAAAQUlEQVR42u3OMQ0AMAwDsAApxL1jN06lMBT5LBmAM/eV7ElJpKWlpaWlpaWlpaWlpaWlpaWlpaWlpaWlpaWlm+kPlHd0NRnvUxsAAAAASUVORK5CYII=";

/** WIDE_PNG's picture as a JPEG stored 60 × 40 with EXIF orientation 6 (rotate 90°): it shows,
 * and the engine crops it, 40 × 60 (D4). */
export const ROTATED_JPEG =
  "/9j/4AAQSkZJRgABAQAAAQABAAD/4QAiRXhpZgAATU0AKgAAAAgAAQESAAMAAAABAAYAAAAAAAD/2wBDAA0JCgsKCA0LCgsODg0PEyAVExISEyccHhcgLikxMC4pLSwzOko+MzZGNywtQFdBRkxOUlNSMj5aYVpQYEpRUk//2wBDAQ4ODhMREyYVFSZPNS01T09PT09PT09PT09PT09PT09PT09PT09PT09PT09PT09PT09PT09PT09PT09PT09PT0//wAARCAAoADwDASIAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwDNooor3TwgqW0/4+U/H+VRVLaf8fKfj/KubG/7tU/wv8jpwf8AvNP/ABL8zRooor4A++CiiigDJooor9IPzgKltP8Aj5T8f5VFUtp/x8p+P8q5sb/u1T/C/wAjpwf+80/8S/M0aKKK+APvgooooAyaKKK/SD84CpbT/j5T8f5UUVzY3/dqn+F/kdOD/wB5p/4l+Zo0UUV8AffBRRRQB//Z";

/** The next native file dialog answers with `data` (base64). */
export const fileDialogAnswers = (page: Page, data: string) =>
  page.evaluate((data) => (window.__dwFixture!.world.tools.files.open = { name: "me.png", data }), data);
