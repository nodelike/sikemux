import fs from "node:fs";
import path from "node:path";

const failed = path.join(process.env.RUNNER_TEMP, "failed-linux-checks");
if (fs.existsSync(failed)) {
  const names = fs.readFileSync(failed, "utf8").trim().split("\n").join(", ");
  console.log(
    `::error::Stopped because ${names} failed, so this job's own checks did not finish`,
  );
}
