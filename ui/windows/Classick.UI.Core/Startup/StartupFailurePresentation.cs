using Classick_UI.Ipc;

namespace Classick_UI.Startup;

public sealed record StartupFailurePresentation(string Title, string Message);

public static class StartupFailurePresentationFactory
{
    public static StartupFailurePresentation For(Exception exception)
    {
        ArgumentNullException.ThrowIfNull(exception);
        return exception is WireCompatibilityException
            ? new StartupFailurePresentation(
                "Classick needs an update",
                "This Classick app and its core use incompatible protocol versions. Update or reinstall Classick so both components come from the same release.")
            : new StartupFailurePresentation(
                "Classick could not start",
                "Classick could not connect to its core. Restart the app. If the problem continues, reinstall Classick.");
    }
}
