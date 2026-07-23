using Classick_UI.Ipc;
using Classick_UI.Startup;

namespace Classick_UI.Tests;

public sealed class StartupFailurePresentationTests
{
    [Fact]
    public void IncompatibleCore_RequiresCoordinatedUpdateWithoutFallback()
    {
        var presentation = StartupFailurePresentationFactory.For(
            new WireCompatibilityException("incompatible wire protocol 2.0.0"));

        Assert.Equal("Classick needs an update", presentation.Title);
        Assert.Contains("incompatible protocol versions", presentation.Message);
        Assert.Contains("same release", presentation.Message);
        Assert.DoesNotContain("fallback", presentation.Message, StringComparison.OrdinalIgnoreCase);
    }
}
